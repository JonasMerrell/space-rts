//! "What does this do?" sheets for planet upgrades and ship classes, opened by tapping a cell in
//! the planet panel. Rules text comes from the sim via `Backend::describe`; the local tables below
//! (mirroring sim formulas at the time of writing) are only a fallback for backends without it.

use super::{GameScreen, Mode};
use crate::art;
use crate::backend::{Backend, UiAction};
use crate::cel;
use crate::input::Input;
use crate::model::*;
use crate::theme::*;
use crate::widgets::{self, Style};
use macroquad::prelude::*;

#[derive(Clone, Debug, PartialEq)]
pub enum Detail {
    Upgrade { body: BodyId, name: String },
    Ship { body: BodyId, class: ShipClass },
    /// The body's build queue.
    Queue { body: BodyId },
}

/// (what it does, effect at `level`) for a planet upgrade.
fn upgrade_text(name: &str, level: u8) -> (&'static str, String) {
    let l = level as f64;
    match name {
        "Mining" => ("Digs matter out of the planet. Matter pays for ships, upgrades and stations.", format!("{:.0} matter/h", 60.0 * (1.0 + l))),
        "Solar" => (
            "Solar panels make energy. Output depends on distance from the star: inner planets get much more.",
            format!("+{:.0} energy/h × sunlight", 60.0 * l),
        ),
        "Reactor" => ("A reactor makes energy anywhere, no matter how far from the star. Same price as solar.", format!("+{:.0} energy/h", 60.0 * l)),
        "Refinery" => (
            "Refines ship fuel (costs 0.5 energy per tonne) and raises the planet's fuel storage. Ships refuel when parked here.",
            format!("{:.0} fuel/h, stores {:.0}", 120.0 * l, 500.0 + 1500.0 * l),
        ),
        "Shipyard" => (
            "Builds ships. Level 1 builds everything except Dreadnoughts, which need level 2.",
            if level == 0 { "no shipyard".into() } else if level == 1 { "small and medium hulls".into() } else { "all hulls incl. Dreadnought".into() },
        ),
        "Defense" => (
            "Planetary guns that fire on hostile ships in orbit and count toward power in orbit (ownership).",
            format!("{:.0} attack, {:.0} hp", 40.0 * l, 250.0 * l),
        ),
        "Telescope" => (
            "Sees farther: reveals ships and planet contents within range. Small ships are harder to spot.",
            format!("{:.0} km range", 60_000.0 + 250_000.0 * l),
        ),
        "Research" => ("A research lab. Higher levels research faster and unlock higher tech tiers.", format!("lab level {level}")),
        _ => ("", String::new()),
    }
}

fn weapon_word(class: ShipClass) -> &'static str {
    match class {
        ShipClass::Interceptor | ShipClass::Bomber => "kinetic (hits hard, slow shells: must lead the target)",
        ShipClass::Gunship | ShipClass::Dreadnought => "energy (light-speed, never misses)",
        _ => "unarmed",
    }
}

/// (role, hp, attack, fuel tank, strong vs, weak vs) mirroring sim `ClassStats` / `vs`.
fn ship_text(class: ShipClass) -> (&'static str, f64, f64, f64, &'static str, &'static str) {
    match class {
        ShipClass::Scout => ("Cheap, small and hard to spot. Use it to look around.", 20.0, 0.0, 30.0, "-", "everything"),
        ShipClass::Interceptor => ("Fast light fighter.", 40.0, 20.0, 60.0, "Bombers, Scouts", "Gunships, Dreadnoughts"),
        ShipClass::Gunship => ("Sturdy line ship with lasers.", 120.0, 30.0, 160.0, "Interceptors", "Bombers"),
        ShipClass::Bomber => ("Heavy hitter against big hulls.", 80.0, 40.0, 130.0, "Gunships, Dreadnoughts", "Interceptors"),
        ShipClass::Dreadnought => ("Huge capital ship. Very visible.", 500.0, 90.0, 700.0, "Gunships, Bombers", "swarms of Bombers"),
        ShipClass::Tanker => ("Carries fuel to other ships and fuel depots.", 60.0, 0.0, 600.0, "-", "everything (takes 1.5× damage)"),
        ShipClass::Constructor => ("Builds space stations where it is parked. More of them build faster.", 50.0, 0.0, 120.0, "-", "everything (takes 1.5× damage)"),
        ShipClass::Decoy => ("Unarmed drone that enemy sensors take for the warship class you pick.", 10.0, 0.0, 40.0, "-", "everything"),
    }
}

fn paragraph(t: &str, x: f32, y: &mut f32, w: f32, size: f32, col: Color) {
    for line in wrap(t, FontKind::Body, size, w) {
        text(&line, x, *y, size, col, FontKind::Body, Align::Left);
        *y += (size + 5.0) * s();
    }
}

fn row(k: &str, v: &str, x: f32, w: f32, y: &mut f32, col: Color) {
    text(k, x, *y, 13.0, MUTED, FontKind::Title, Align::Left);
    text(v, x + w, *y, 14.0, col, FontKind::Body, Align::Right);
    *y += 22.0 * s();
}

/// Draws the open detail sheet in place of the planet panel. Returns false if it should close.
pub fn draw(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, view: &GameView, info: &BodyInfo, r: Rect) -> bool {
    let Some(d) = g.detail.clone() else { return false };
    inp.block(r);
    cel::panel(r, alpha(PANEL, 0.98));
    let pad = 16.0 * s();
    let (x, w) = (r.x + pad, r.w - 2.0 * pad);
    let mut y = r.y + 24.0 * s();
    let res = view.resources;
    let afford = |c: &Cost| res.matter >= c.matter && res.energy >= c.energy;
    let cost_s = |c: &Cost| format!("{} matter · {} energy · {}", fmt_amount(c.matter), fmt_amount(c.energy), fmt_dur(c.seconds));
    let mut action: Option<(String, bool, UiAction)> = None;
    match &d {
        Detail::Queue { body } => {
            inked("BUILD QUEUE", x, y, 22.0, PAPER, Align::Left);
            let total = info.queue.last().map(|q| q.1 - view.now).unwrap_or(0.0);
            text(&format!("{} item(s) · all done in {}", info.queue.len() + info.waiting.len(), fmt_dur(total.max(0.0))), x + w, y, 13.0, MUTED, FontKind::Title, Align::Right);
            y += 26.0 * s();
            if info.queue.is_empty() && info.waiting.is_empty() {
                paragraph("Nothing queued. Tap + on the Shipyard or Upgrades tab to build.", x, &mut y, w, 14.0, MUTED);
            }
            let player = view.player(view.me);
            let skin = player.map(|p| art::skin_of(&p.faction)).unwrap_or("vanguard");
            let row_h = 46.0 * s();
            let mut max_rows = (((r.y + r.h - 64.0 * s()) - y) / row_h).floor().max(1.0) as usize;
            // leave room for the waiting-for-funds list
            if !info.waiting.is_empty() {
                max_rows = max_rows.saturating_sub(info.waiting.len().min(3) + 1).max(1);
            }
            let mut start = view.now;
            for (i, (label, done)) in info.queue.iter().enumerate().take(max_rows) {
                let class = ShipClass::ALL.iter().copied().find(|c| c.label() == label.as_str());
                // Job i runs after job i-1; the first has been running for (its build time - remaining).
                let dur = class
                    .and_then(|c| info.buildable.iter().find(|b| b.0 == c))
                    .map(|b| b.1.seconds)
                    .unwrap_or((done - start).max(1.0));
                let begin = if i == 0 { done - dur } else { start };
                let frac = ((view.now - begin) / (done - begin).max(1.0)).clamp(0.0, 1.0) as f32;
                start = *done;
                let rr = Rect::new(x - 6.0 * s(), y, w + 12.0 * s(), row_h - 6.0 * s());
                cel::round_rect(rr, 6.0 * s(), Color::new(1.0, 1.0, 1.0, if i == 0 { 0.08 } else { 0.04 }));
                let ic = vec2(rr.x + 20.0 * s(), rr.center().y);
                match class.and_then(|c| art::ship_tex(c, skin, player.map(|p| p.color))) {
                    Some(t) => art::draw_centered(&t, ic, vec2(30.0, 30.0) * s(), -0.5, WHITE),
                    None => {
                        if let Some(t) = art::tex("sprites/icons/upgrades.png") {
                            art::draw_centered(&t, ic, vec2(24.0, 24.0) * s(), 0.0, WHITE);
                        }
                    }
                }
                text(&format!("{}. {}", i + 1, label.to_uppercase()), rr.x + 40.0 * s(), rr.y + 13.0 * s(), 13.0, PAPER, FontKind::Title, Align::Left);
                let eta = if i == 0 { format!("{} left", fmt_dur(done - view.now)) } else { format!("ready in {}", fmt_dur(done - view.now)) };
                text(&eta, rr.x + rr.w - 112.0 * s(), rr.y + 13.0 * s(), 12.0, MUTED, FontKind::Body, Align::Right);
                widgets::bar(Rect::new(rr.x + 40.0 * s(), rr.y + 26.0 * s(), rr.w - 156.0 * s(), 6.0 * s()), frac, if i == 0 { ACCENT } else { Color::new(1.0, 1.0, 1.0, 0.25) });
                // reorder: up / down
                let n = info.queue.len();
                for (k, (dir, to)) in [(-1.0f32, i.checked_sub(1)), (1.0, (i + 1 < n).then_some(i + 1))].into_iter().enumerate() {
                    let br = Rect::new(rr.x + rr.w - 100.0 * s() + k as f32 * 34.0 * s(), rr.y + 5.0 * s(), 30.0 * s(), rr.h - 10.0 * s());
                    let Some(to) = to else { continue };
                    if widgets::button(inp, br, "", Style::Secondary) {
                        art::sfx("menu_select");
                        g.emit(UiAction::MoveJob { body: *body, from: i, to });
                    }
                    let c = br.center();
                    // chevron: apex up for "earlier", down for "later"
                    let a = 3.5 * s() * dir;
                    draw_line(c.x - 6.0 * s(), c.y - a, c.x, c.y + a, 2.0 * s(), PAPER);
                    draw_line(c.x, c.y + a, c.x + 6.0 * s(), c.y - a, 2.0 * s(), PAPER);
                }
                let xr = vec2(rr.x + rr.w - 20.0 * s(), rr.center().y);
                if widgets::round_button(inp, xr, 11.0 * s(), "X", PAPER, 0) {
                    art::sfx("menu_close");
                    g.emit(UiAction::CancelJob { body: *body, index: i });
                }
                y += row_h;
            }
            if info.queue.len() > max_rows {
                text(&format!("+{} more", info.queue.len() - max_rows), x, y + 8.0 * s(), 13.0, MUTED, FontKind::Body, Align::Left);
                y += 22.0 * s();
            }
            if !info.waiting.is_empty() {
                waiting_rows(g, inp, view, info, x, w, &mut y, r.y + r.h - 80.0 * s());
            }
            let foot = if info.waiting.is_empty() { "Cancelling refunds the cost." } else { "Waiting items start by themselves once you can pay, top first." };
            text(foot, x, r.y + r.h - 70.0 * s(), 12.0, MUTED, FontKind::Body, Align::Left);
        }
        Detail::Upgrade { body, name } => {
            let Some(u) = info.upgrades.iter().find(|u| &u.name == name) else { return false };
            let (what, now) = upgrade_text(name, u.level);
            inked(&name.to_uppercase(), x, y, 22.0, PAPER, Align::Left);
            for k in 0..u.max {
                let pc = vec2(x + w - (u.max - k) as f32 * 14.0 * s() + 6.0 * s(), y);
                draw_circle(pc.x, pc.y, 5.0 * s(), if k < u.level { ACCENT } else { Color::new(1.0, 1.0, 1.0, 0.18) });
            }
            y += 30.0 * s();
            paragraph(what, x, &mut y, w, 14.0, PAPER);
            y += 8.0 * s();
            let sim = |l: u8| be.describe(&format!("upgrade:{name}:{l}"));
            match (u.level, sim(u.level)) {
                (0, _) => row("LEVEL 0", "not built", x, w, &mut y, PAPER),
                (l, Some(t)) => {
                    text(&format!("NOW · LEVEL {l}"), x, y, 13.0, MUTED, FontKind::Title, Align::Left);
                    y += 20.0 * s();
                    paragraph(&t, x, &mut y, w, 13.0, PAPER);
                }
                (l, None) => row(&format!("LEVEL {l}"), &now, x, w, &mut y, PAPER),
            }
            match &u.next {
                Some(c) => {
                    match sim(u.level + 1) {
                        Some(t) => {
                            text(&format!("NEXT · LEVEL {}", u.level + 1), x, y, 13.0, MUTED, FontKind::Title, Align::Left);
                            y += 20.0 * s();
                            paragraph(&t, x, &mut y, w, 13.0, GOOD);
                        }
                        None => {
                            let (_, next) = upgrade_text(name, u.level + 1);
                            row(&format!("LEVEL {}", u.level + 1), &next, x, w, &mut y, GOOD);
                        }
                    }
                    row("COST", &cost_s(c), x, w, &mut y, if afford(c) { PAPER } else { DANGER });
                    let label = if !afford(c) { "QUEUE" } else if u.level == 0 { "BUILD" } else { "UPGRADE" };
                    action = Some((label.into(), afford(c), UiAction::Upgrade { body: *body, upgrade: name.clone() }));
                }
                None => row("", "fully upgraded", x, w, &mut y, GOOD),
            }
        }
        Detail::Ship { body, class } => {
            let (role, hp, atk, tank, strong, weak) = ship_text(*class);
            let player = view.player(view.me);
            let skin = player.map(|p| art::skin_of(&p.faction)).unwrap_or("vanguard");
            if let Some(t) = art::ship_tex(*class, skin, player.map(|p| p.color)) {
                art::draw_centered(&t, vec2(x + w - 26.0 * s(), y + 6.0 * s()), vec2(56.0, 56.0) * s(), -0.4, WHITE);
            }
            inked(&class.label().to_uppercase(), x, y, 22.0, PAPER, Align::Left);
            y += 30.0 * s();
            paragraph(role, x, &mut y, w - 60.0 * s(), 14.0, PAPER);
            y += 6.0 * s();
            if let Some(t) = be.describe(&format!("ship:{}", class.label())) {
                // "Gunship — 120 hull, ..." → drop the repeated name.
                let t = t.split_once(" — ").map(|(_, r)| r.to_string()).unwrap_or(t);
                paragraph(&t, x, &mut y, w, 13.0, PAPER);
                y += 4.0 * s();
            } else {
                row("HULL", &format!("{hp:.0} hp"), x, w, &mut y, PAPER);
                let weapon = weapon_word(*class);
                row("WEAPON", &if atk > 0.0 { format!("{atk:.0} attack · {}", weapon.split(' ').next().unwrap_or("")) } else { "none".into() }, x, w, &mut y, PAPER);
                row("FUEL TANK", &format!("{tank:.0}"), x, w, &mut y, PAPER);
                row("STRONG VS", strong, x, w, &mut y, GOOD);
                row("WEAK VS", weak, x, w, &mut y, DANGER);
                if atk > 0.0 {
                    if let Some((_, why)) = weapon.split_once(' ') {
                        paragraph(why.trim_matches(|c| c == '(' || c == ')'), x, &mut y, w, 12.0, MUTED);
                    }
                }
            }
            if let Some((_, c)) = info.buildable.iter().find(|(k, _)| k == class) {
                row("COST", &cost_s(c), x, w, &mut y, if afford(c) { PAPER } else { DANGER });
                action = Some((if afford(c) { "BUILD" } else { "QUEUE" }.into(), afford(c), UiAction::Build { body: *body, class: *class }));
            } else {
                row("", "this shipyard can't build it yet", x, w, &mut y, MUTED);
            }
        }
    }
    let by = r.y + r.h - 54.0 * s();
    let bw = (r.w - 3.0 * 12.0 * s()) / 2.0;
    let mut keep = true;
    if let Some((label, ok, a)) = action {
        // can't afford it yet: QUEUE, it starts once income covers it
        if widgets::button(inp, Rect::new(r.x + 12.0 * s(), by, bw, 44.0 * s()), &label, if ok { Style::Primary } else { Style::Secondary }) {
            art::sfx(if ok { "purchase" } else { "menu_select" });
            if !ok {
                g.hint = Some(("Queued: starts when you can afford it".into(), get_time()));
            }
            g.emit(a);
            keep = false;
        }
    }
    if widgets::button(inp, Rect::new(r.x + 24.0 * s() + bw, by, bw, 44.0 * s()), "BACK", Style::Ghost) {
        keep = false;
    }
    if !keep || !matches!(g.mode, Mode::Idle) {
        g.detail = None;
    }
    true
}

/// "WAITING FOR FUNDS" rows in the build queue sheet: projected start, reorder, cancel.
#[allow(clippy::too_many_arguments)]
fn waiting_rows(g: &mut GameScreen, inp: &mut Input, view: &GameView, info: &BodyInfo, x: f32, w: f32, y: &mut f32, bottom: f32) {
    text("WAITING FOR FUNDS", x, *y + 6.0 * s(), 13.0, MUTED, FontKind::Title, Align::Left);
    *y += 20.0 * s();
    let row_h = 46.0 * s();
    let n = info.waiting.len();
    for (k, it) in info.waiting.iter().enumerate() {
        if *y + row_h > bottom {
            text(&format!("+{} more waiting", n - k), x, *y + 8.0 * s(), 13.0, MUTED, FontKind::Body, Align::Left);
            break;
        }
        let rr = Rect::new(x - 6.0 * s(), *y, w + 12.0 * s(), row_h - 6.0 * s());
        cel::round_rect(rr, 6.0 * s(), Color::new(1.0, 1.0, 1.0, 0.03));
        cel::round_rect_lines(rr, 6.0 * s(), (1.0 * s()).max(1.0), Color::new(1.0, 1.0, 1.0, 0.12));
        if let Some(t) = art::tex("sprites/icons/time.png") {
            art::draw_centered(&t, vec2(rr.x + 20.0 * s(), rr.center().y), vec2(22.0, 22.0) * s(), 0.0, Color::new(1.0, 1.0, 1.0, 0.7));
        }
        let mut title = it.label.to_uppercase();
        if it.of > 1 {
            title = format!("{title}  #{}/{}", it.index + 1, it.of);
        }
        text(&title, rr.x + 40.0 * s(), rr.y + 13.0 * s(), 13.0, PAPER, FontKind::Title, Align::Left);
        let (sub, col) = match (&it.blocked, it.start_t) {
            (Some(why), _) => (why.clone(), DANGER),
            (None, Some(t)) if t <= view.now + 1.0 => ("starting".to_string(), GOOD),
            (None, Some(t)) => (format!("starts in ~{} · {}M {}E", fmt_dur(t - view.now), fmt_amount(it.cost.matter), fmt_amount(it.cost.energy)), MUTED),
            (None, None) => ("no income for it yet".to_string(), DANGER),
        };
        let sub = wrap(&sub, FontKind::Body, 12.0, rr.w - 156.0 * s()).into_iter().next().unwrap_or_default();
        text(&sub, rr.x + 40.0 * s(), rr.y + 30.0 * s(), 12.0, col, FontKind::Body, Align::Left);
        // reorder within this body's waiting items (positions are in the whole list)
        let prev = k.checked_sub(1).map(|j| info.waiting[j].index);
        let next = info.waiting.get(k + 1).map(|w| w.index);
        for (b, (dir, to)) in [(-1.0f32, prev), (1.0, next)].into_iter().enumerate() {
            let br = Rect::new(rr.x + rr.w - 100.0 * s() + b as f32 * 34.0 * s(), rr.y + 5.0 * s(), 30.0 * s(), rr.h - 10.0 * s());
            let Some(to) = to else { continue };
            if widgets::button(inp, br, "", Style::Secondary) {
                art::sfx("menu_select");
                g.emit(UiAction::MoveWait { id: it.id, to });
            }
            let c = br.center();
            let a = 3.5 * s() * dir;
            draw_line(c.x - 6.0 * s(), c.y - a, c.x, c.y + a, 2.0 * s(), PAPER);
            draw_line(c.x, c.y + a, c.x + 6.0 * s(), c.y - a, 2.0 * s(), PAPER);
        }
        let xr = vec2(rr.x + rr.w - 20.0 * s(), rr.center().y);
        if widgets::round_button(inp, xr, 11.0 * s(), "X", PAPER, 0) {
            art::sfx("menu_close");
            g.emit(UiAction::Unwait { id: it.id });
        }
        *y += row_h;
    }
}

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
                    let label = if u.level == 0 { "BUILD" } else { "UPGRADE" };
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
                action = Some(("BUILD".into(), afford(c), UiAction::Build { body: *body, class: *class }));
            } else {
                row("", "this shipyard can't build it yet", x, w, &mut y, MUTED);
            }
        }
    }
    let by = r.y + r.h - 54.0 * s();
    let bw = (r.w - 3.0 * 12.0 * s()) / 2.0;
    let mut keep = true;
    if let Some((label, ok, a)) = action {
        if widgets::button(inp, Rect::new(r.x + 12.0 * s(), by, bw, 44.0 * s()), &label, if ok { Style::Primary } else { Style::Ghost }) {
            if ok {
                art::sfx("purchase");
                g.emit(a);
                keep = false;
            } else {
                art::sfx("ui_error");
            }
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

//! Queues overview: every research project, shipyard job, upgrade and waiting-for-funds item across
//! everything we own on one scrollable screen (QUEUES button in the HUD). Tapping a body jumps there.

use super::map;
use super::GameScreen;
use crate::art;
use crate::backend::Backend;
use crate::cel;
use crate::input::Input;
use crate::model::*;
use crate::screens::{safe_bottom, safe_top};
use crate::theme::*;
use crate::widgets::{self, Style};
use macroquad::prelude::*;

/// One line of the list.
enum Row {
    /// Group header; Some(body) for a body (tappable), None for RESEARCH.
    Head { body: Option<BodyId>, title: String, sub: String },
    /// Something being built or researched (running or lined up behind the running one).
    Job { body: BodyId, lane: &'static str, label: String, ship: Option<ShipClass>, start_t: f64, done_t: f64 },
    /// Queued but not paid for yet.
    Waiting { body: BodyId, item: WaitingItem },
}

impl Row {
    fn height(&self) -> f32 {
        match self {
            Row::Head { .. } => 52.0,
            _ => 48.0,
        }
    }

    fn body(&self) -> Option<BodyId> {
        match self {
            Row::Head { body, .. } => *body,
            Row::Job { body, .. } | Row::Waiting { body, .. } => Some(*body),
        }
    }

    /// Which tab of the body sheet to open on: 1 shipyard, 0 upgrades.
    fn tab(&self) -> usize {
        match self {
            Row::Job { ship: Some(_), .. } => 1,
            _ => 0,
        }
    }
}

pub fn hud_button(g: &mut GameScreen, inp: &mut Input, c: Vec2) {
    if widgets::round_button(inp, c, 21.0 * s(), "@build", PAPER, 0) {
        g.queues_open = !g.queues_open;
        g.queues_scroll = 0.0;
        art::sfx(if g.queues_open { "menu_open" } else { "menu_close" });
    }
}

/// Lays out the whole list: research first, then each body with anything going on.
/// Also returns the owned bodies with nothing queued.
fn rows(be: &dyn Backend, view: &GameView) -> (Vec<Row>, Vec<BodyId>) {
    let rv = be.research();
    let tech_names: Vec<&str> = rv.as_ref().map(|r| r.techs.iter().flat_map(|t| [t.name.as_str(), t.id.as_str()]).collect()).unwrap_or_default();
    let mut research: Vec<Row> = vec![];
    if let Some(r) = &rv {
        let mut running: Vec<&TechInfo> = r.techs.iter().filter(|t| matches!(t.state, TechState::Running { .. })).collect();
        running.sort_by(|a, b| done_of(a).total_cmp(&done_of(b)));
        for t in running {
            let TechState::Running { lab, done_t } = t.state else { continue };
            research.push(Row::Job { body: lab, lane: "", label: t.name.clone(), ship: None, start_t: (done_t - t.cost.seconds).min(view.now), done_t });
        }
    }
    let mut bodies: Vec<Row> = vec![];
    let mut idle = vec![];
    let mut research_waits: Vec<Row> = vec![];
    for b in &view.bodies {
        let Some(info) = be.body_info(b.id) else { continue };
        let mine = b.owner == Some(view.me);
        // research waiting for funds sits at its lab; it goes in the research group instead
        let (res_w, rest): (Vec<&WaitingItem>, Vec<&WaitingItem>) = info.waiting.iter().partition(|w| tech_names.contains(&w.label.as_str()));
        research_waits.extend(res_w.into_iter().map(|w| {
            let mut item = w.clone();
            if let Some(t) = rv.as_ref().and_then(|r| r.techs.iter().find(|t| t.id == w.label)) {
                item.label = t.name.clone();
            }
            Row::Waiting { body: b.id, item }
        }));
        // a body we don't own only shows when we have something going there (waiting station etc.)
        let jobs: Vec<&QueueItem> = if mine { info.queue.iter().collect() } else { vec![] };
        if jobs.is_empty() && rest.is_empty() {
            if mine {
                idle.push(b.id);
            }
            continue;
        }
        let building = jobs.iter().filter(|q| q.start_t <= view.now).count();
        let mut sub = vec![];
        if building > 0 {
            sub.push(format!("{building} building"));
        }
        if jobs.len() > building {
            sub.push(format!("{} queued", jobs.len() - building));
        }
        if !rest.is_empty() {
            sub.push(format!("{} waiting for funds", rest.len()));
        }
        bodies.push(Row::Head { body: Some(b.id), title: format!("{} · {}", b.name.to_uppercase(), kind_name(b.kind)), sub: sub.join(" · ") });
        // the two lanes build side by side: shipyard first, then upgrades, each in its own order
        for upgrades in [false, true] {
            for q in jobs.iter().filter(|q| q.upgrade() == upgrades) {
                let lane = if upgrades { "UPGRADE" } else { "SHIPYARD" };
                bodies.push(Row::Job { body: b.id, lane, label: q.label.clone(), ship: q.ship, start_t: q.start_t, done_t: q.done_t });
            }
        }
        bodies.extend(rest.into_iter().map(|w| Row::Waiting { body: b.id, item: w.clone() }));
    }
    let mut out = vec![];
    if !research.is_empty() || !research_waits.is_empty() {
        let mut sub = vec![];
        if !research.is_empty() {
            sub.push(format!("{} researching", research.len()));
        }
        if !research_waits.is_empty() {
            sub.push(format!("{} waiting", research_waits.len()));
        }
        out.push(Row::Head { body: None, title: "RESEARCH".into(), sub: sub.join(" · ") });
        out.extend(research);
        out.extend(research_waits);
    }
    out.extend(bodies);
    (out, idle)
}

fn done_of(t: &TechInfo) -> f64 {
    match t.state {
        TechState::Running { done_t, .. } => done_t,
        _ => f64::INFINITY,
    }
}

fn kind_name(k: BodyKind) -> &'static str {
    match k {
        BodyKind::Star => "STAR",
        BodyKind::Planet => "PLANET",
        BodyKind::Moon => "MOON",
        BodyKind::Asteroid => "ASTEROID",
        BodyKind::Outpost => "OUTPOST",
    }
}

pub fn queues_screen(g: &mut GameScreen, inp: &mut Input, be: &dyn Backend, view: &GameView) {
    let m = 12.0 * s();
    let rw = (screen_width() - 2.0 * m).min(760.0 * s());
    let r = Rect::new((screen_width() - rw) / 2.0, safe_top() + m, rw, screen_height() - safe_top() - safe_bottom() - 2.0 * m);
    inp.block(r);
    cel::panel(r, alpha(PANEL, 0.98));
    cel::round_rect_lines(r, 14.0 * s(), 1.0 * s(), BORDER);
    text("QUEUES", r.x + 18.0 * s(), r.y + 26.0 * s(), 22.0, PAPER, FontKind::Title, Align::Left);
    let (rows, idle) = rows(be, view);
    let jobs = rows.iter().filter(|r| matches!(r, Row::Job { .. })).count();
    let waits = rows.iter().filter(|r| matches!(r, Row::Waiting { .. })).count();
    let last = rows.iter().filter_map(|r| if let Row::Job { done_t, .. } = r { Some(*done_t) } else { None }).fold(view.now, f64::max);
    let sub = if jobs + waits == 0 {
        "Nothing queued anywhere".to_string()
    } else {
        let mut v = vec![format!("{jobs} building or lined up")];
        if waits > 0 {
            v.push(format!("{waits} waiting"));
        }
        if last > view.now {
            v.push(format!("done in {}", fmt_dur(last - view.now)));
        }
        v.join(" · ")
    };
    text(&sub, r.x + 18.0 * s(), r.y + 50.0 * s(), 14.0, MUTED, FontKind::Body, Align::Left);
    let mut shut = widgets::round_button(inp, vec2(r.x + r.w - 24.0 * s(), r.y + 24.0 * s()), 14.0 * s(), "X", PAPER, 0);

    let bh = 50.0 * s();
    let br = Rect::new(r.x + 16.0 * s(), r.y + r.h - bh - 14.0 * s(), r.w - 32.0 * s(), bh);
    shut |= widgets::button(inp, br, "TO THE MAP", Style::Ghost);
    shut |= inp.take_back();

    // list, scrolled by dragging (or the mouse wheel) inside it
    let list = Rect::new(r.x + 10.0 * s(), r.y + 66.0 * s(), r.w - 20.0 * s(), br.y - r.y - 76.0 * s());
    if let (Some(d), Some(p)) = (inp.drag, inp.drag_pos) {
        if list.contains(p) {
            g.queues_scroll -= d.y;
        }
    }
    if let Some((c, k)) = inp.zoom {
        if list.contains(c) {
            g.queues_scroll -= k.ln() / 1.15f32.ln() * 60.0 * s();
        }
    }
    let idle_names: Vec<String> = idle.iter().filter_map(|&b| view.body(b)).map(|b| b.name.clone()).collect();
    let idle_lines = if idle_names.is_empty() { vec![] } else { wrap(&format!("Idle: {}", idle_names.join(", ")), FontKind::Body, 13.0, list.w - 16.0 * s()) };
    let gap = 6.0 * s();
    let h: f32 = rows.iter().map(|r| r.height() * s() + gap).sum::<f32>() + if idle_lines.is_empty() { 0.0 } else { (14.0 + 18.0 * idle_lines.len() as f32) * s() };
    g.queues_scroll = g.queues_scroll.clamp(0.0, (h - list.h).max(0.0));

    let player = view.player(view.me);
    let skin = player.map(|p| art::skin_of(&p.faction)).unwrap_or("vanguard");
    let time = get_time() as f32;
    let mut tapped: Option<&Row> = None;
    let mut y = list.y - g.queues_scroll;
    cel::clip(Some(list));
    for row in &rows {
        let rh = row.height() * s();
        let rr = Rect::new(list.x, y, list.w, rh);
        y += rh + gap;
        if rr.y + rr.h < list.y || rr.y > list.y + list.h {
            continue;
        }
        match row {
            Row::Head { body, title, sub } => {
                let tap = body.is_some();
                cel::panel(rr, alpha(PANEL_HI, if tap && inp.holding(rr) { 0.95 } else { 0.7 }));
                let ic = vec2(rr.x + 28.0 * s(), rr.center().y);
                match body.and_then(|b| view.body(b)) {
                    Some(b) => map::draw_body(b, ic, 12.0 * s(), ic + vec2(-200.0, -80.0), view.color_of(b.owner), time),
                    None => {
                        if let Some(t) = art::tex("sprites/icons/research.png") {
                            art::draw_centered(&t, ic, vec2(26.0, 26.0) * s(), 0.0, WHITE);
                        }
                    }
                }
                inked(title, rr.x + 60.0 * s(), rr.y + 18.0 * s(), 16.0, PAPER, Align::Left);
                text(sub, rr.x + 60.0 * s(), rr.y + 38.0 * s(), 12.0, MUTED, FontKind::Body, Align::Left);
                if tap {
                    text("SHOW ›", rr.x + rr.w - 12.0 * s(), rr.center().y, 13.0, ACCENT, FontKind::Title, Align::Right);
                }
            }
            Row::Job { lane, label, ship, start_t, done_t, .. } => {
                let running = *start_t <= view.now;
                let frac = ((view.now - start_t) / (done_t - start_t).max(1.0)).clamp(0.0, 1.0) as f32;
                let jr = Rect::new(rr.x + 14.0 * s(), rr.y, rr.w - 14.0 * s(), rr.h);
                cel::round_rect(jr, 6.0 * s(), Color::new(1.0, 1.0, 1.0, if running { 0.08 } else { 0.04 }));
                let ic = vec2(jr.x + 22.0 * s(), jr.center().y);
                let icon = match ship {
                    Some(c) => art::ship_tex(*c, skin, player.map(|p| p.color)),
                    None if lane.is_empty() => art::tex("sprites/icons/research.png"),
                    None => art::tex("sprites/icons/upgrades.png"),
                };
                if let Some(t) = icon {
                    let px = if ship.is_some() { 32.0 } else { 24.0 };
                    art::draw_centered(&t, ic, vec2(px, px) * s(), if ship.is_some() { -0.5 } else { 0.0 }, WHITE);
                }
                let tx = jr.x + 44.0 * s();
                text(&label.to_uppercase(), tx, jr.y + 15.0 * s(), 13.0, PAPER, FontKind::Title, Align::Left);
                if !lane.is_empty() {
                    let lw = measure(&label.to_uppercase(), FontKind::Title, 13.0).x;
                    text(lane, tx + lw + 8.0 * s(), jr.y + 15.0 * s(), 10.0, MUTED, FontKind::Title, Align::Left);
                }
                let left = (done_t - view.now).max(0.0);
                let (eta, col) = if running { (format!("{} left", fmt_dur(left)), ACCENT) } else { (format!("queued · ready in {}", fmt_dur(left)), MUTED) };
                text(&eta, jr.x + jr.w - 10.0 * s(), jr.y + 15.0 * s(), 12.0, col, FontKind::Body, Align::Right);
                widgets::bar(Rect::new(tx, jr.y + 30.0 * s(), jr.w - 44.0 * s() - 10.0 * s(), 6.0 * s()), frac, if running { ACCENT } else { Color::new(1.0, 1.0, 1.0, 0.25) });
            }
            Row::Waiting { item, .. } => {
                let jr = Rect::new(rr.x + 14.0 * s(), rr.y, rr.w - 14.0 * s(), rr.h);
                cel::round_rect(jr, 6.0 * s(), Color::new(1.0, 1.0, 1.0, 0.03));
                let edge = if item.blocked.is_some() || item.start_t.is_none() { alpha(DANGER, 0.6) } else { alpha(ACCENT, 0.45) };
                cel::round_rect_lines(jr, 6.0 * s(), (1.0 * s()).max(1.0), edge);
                if let Some(t) = art::tex("sprites/icons/time.png") {
                    art::draw_centered(&t, vec2(jr.x + 22.0 * s(), jr.center().y), vec2(22.0, 22.0) * s(), 0.0, Color::new(1.0, 1.0, 1.0, 0.7));
                }
                let tx = jr.x + 44.0 * s();
                text(&item.label.to_uppercase(), tx, jr.y + 15.0 * s(), 13.0, PAPER, FontKind::Title, Align::Left);
                text("WAITING FOR FUNDS", jr.x + jr.w - 10.0 * s(), jr.y + 15.0 * s(), 11.0, ACCENT, FontKind::Title, Align::Right);
                let (sub, col) = match (&item.blocked, item.start_t) {
                    (Some(why), _) => (why.clone(), DANGER),
                    (None, Some(t)) if t <= view.now + 1.0 => ("starting".to_string(), GOOD),
                    (None, Some(t)) => (format!("needs {}M {}E · starts in ~{}", fmt_amount(item.cost.matter), fmt_amount(item.cost.energy), fmt_dur(t - view.now)), MUTED),
                    (None, None) => (format!("needs {}M {}E · no income for it yet", fmt_amount(item.cost.matter), fmt_amount(item.cost.energy)), DANGER),
                };
                let sub = wrap(&sub, FontKind::Body, 12.0, jr.w - 54.0 * s()).into_iter().next().unwrap_or_default();
                text(&sub, tx, jr.y + 33.0 * s(), 12.0, col, FontKind::Body, Align::Left);
            }
        }
        let visible = Rect::new(rr.x, rr.y.max(list.y), rr.w, (rr.y + rr.h).min(list.y + list.h) - rr.y.max(list.y));
        if row.body().is_some() && visible.h > 0.0 && inp.tapped(visible) {
            inp.consume_tap();
            tapped = Some(row);
        }
    }
    if rows.is_empty() {
        let msg = "Nothing queued. Open a planet or moon you own and tap + on its Shipyard or Upgrades tab, or start a project on the Research screen.";
        let lines = wrap(msg, FontKind::Body, 14.0, list.w - 16.0 * s());
        for (k, l) in lines.iter().enumerate() {
            text(l, list.x + 8.0 * s(), y + (14.0 + 18.0 * k as f32) * s(), 14.0, MUTED, FontKind::Body, Align::Left);
        }
        y += (14.0 + 18.0 * lines.len() as f32) * s();
    }
    for (k, l) in idle_lines.iter().enumerate() {
        text(l, list.x + 8.0 * s(), y + (10.0 + 18.0 * k as f32) * s(), 13.0, MUTED, FontKind::Body, Align::Left);
    }
    cel::clip(None);
    if let Some(row) = tapped {
        let body = row.body().unwrap();
        art::sfx("menu_select");
        g.queues_open = false;
        g.mode = super::Mode::Idle;
        g.focus(be, Focus::Body(body), view.now + g.scrub);
        g.follow = true;
        g.system_view = false;
        g.body_tab = row.tab();
        if let Some(b) = view.body(body) {
            g.zoom_goal = Some(super::panels::zoom_to_body(view, b));
        }
    } else if shut {
        art::sfx("menu_close");
        g.queues_open = false;
    }
}

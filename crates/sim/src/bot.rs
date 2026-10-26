//! Computer opponents. Deterministic (they run inside `Game::advance_to` on the server), so a
//! replay of the same commands gives the same game.
use crate::game::{Command, Game, PlayerId, ShipClass, ShipState, Upgrade, TICK};
use crate::system::{BodyId, BodyKind};
use crate::tech::{Branch, Tech};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Difficulty {
    Easy,
    Normal,
    Hard,
}

impl Difficulty {
    pub fn parse(s: &str) -> Difficulty {
        match s.to_ascii_lowercase().as_str() {
            "easy" => Difficulty::Easy,
            "hard" => Difficulty::Hard,
            _ => Difficulty::Normal,
        }
    }
    /// Minutes between decisions.
    fn period(self) -> u64 {
        match self {
            Difficulty::Easy => 90,
            Difficulty::Normal => 30,
            Difficulty::Hard => 10,
        }
    }
    fn squad(self) -> usize {
        match self {
            Difficulty::Easy => 2,
            Difficulty::Normal => 3,
            Difficulty::Hard => 3,
        }
    }
}

pub fn think_all(g: &mut Game) {
    let minute = (g.last_tick / TICK).round() as u64;
    let bots: Vec<(PlayerId, Difficulty)> =
        g.players.iter().filter_map(|p| p.bot.as_deref().map(|d| (p.id, Difficulty::parse(d)))).collect();
    for (id, d) in bots {
        // stagger bots so they don't all plan in the same tick
        if (minute + id as u64 * 7) % d.period() == 0 {
            think(g, id, d);
        }
    }
}

fn think(g: &mut Game, me: PlayerId, d: Difficulty) {
    let now = g.t;
    g.players[me as usize].bot_avoid.retain(|a| a.2 > now);
    let owned: Vec<BodyId> = (0..g.bodies.len()).filter(|&i| g.bodies[i].owner == Some(me)).map(|i| BodyId(i as u32)).collect();
    if owned.is_empty() {
        return;
    }
    // economy first: mining and power at home, then shipyards
    let home = g.players[me as usize].home.filter(|h| owned.contains(h)).unwrap_or(owned[0]);
    for u in [Upgrade::Mining, Upgrade::Solar, Upgrade::Reactor, Upgrade::Refinery, Upgrade::Defense, Upgrade::Shipyard, Upgrade::Research] {
        let b = &g.bodies[home.0 as usize];
        if b.queue.is_empty() && b.level(u) < 2 {
            let _ = g.apply(me, Command::Upgrade { body: home, upgrade: u });
            break;
        }
    }
    // research: industry and weapons first, scanning last; easy bots don't research
    if d != Difficulty::Easy {
        let order = [Branch::Industry, Branch::Weapons, Branch::Defense, Branch::Engines, Branch::Scanning];
        let next = (1..=4u8)
            .flat_map(|tier| order.iter().flat_map(move |&b| Tech::ALL.into_iter().filter(move |t| t.branch() == b && t.tier() == tier)))
            .find(|&t| g.research_blocker(me, home, t).is_none());
        if let Some(t) = next {
            let _ = g.apply(me, Command::Research { body: home, tech: t });
        }
    }
    let mix = [ShipClass::Gunship, ShipClass::Bomber, ShipClass::Interceptor, ShipClass::Gunship, ShipClass::Tanker];
    let n_ships = g.ships.iter().filter(|s| s.owner == me).count();
    let want = mix[n_ships % mix.len()];
    if g.bodies[home.0 as usize].queue.len() < 2 {
        let _ = g.apply(me, Command::Build { body: home, class: want });
    }
    // expansion / attack: idle armed ships at home go out in squads
    let idle: Vec<_> = g
        .ships
        .iter()
        .filter(|s| s.owner == me && s.class.stats().attack > 0.0)
        .filter(|s| matches!(s.state, ShipState::Parked { departure: None, .. }) && s.route.is_empty())
        .filter(|s| s.parked_at().is_some_and(|b| g.bodies[b.0 as usize].owner == Some(me)))
        .map(|s| (s.id, s.parked_at().unwrap()))
        .collect();
    let squad = d.squad();
    let mut by_body: Vec<(BodyId, Vec<_>)> = vec![];
    for (id, b) in idle {
        match by_body.iter_mut().find(|e| e.0 == b) {
            Some(e) => e.1.push(id),
            None => by_body.push((b, vec![id])),
        }
    }
    for (from, ships) in by_body {
        // keep a garrison of one at home
        let spare = if from == home { ships.len().saturating_sub(1) } else { ships.len() };
        if spare < squad {
            continue;
        }
        // refuel before heading out
        let go: Vec<_> = ships.into_iter().filter(|id| g.ship(*id).is_some_and(|s| s.dv_left() > 12.0)).take(spare).collect();
        if go.len() < squad {
            continue;
        }
        let Some(target) = pick_target(g, me, from, d) else { continue };
        if g.apply(me, Command::Transfer { ships: go, to: target, depart_t: None }).is_err() {
            let until = g.t + 6.0 * 3600.0;
            g.players[me as usize].bot_avoid.push((from, target, until));
        }
    }
}

fn pick_target(g: &Game, me: PlayerId, from: BodyId, d: Difficulty) -> Option<BodyId> {
    let here = g.system.pos(from, g.t);
    g.system
        .bodies
        .iter()
        .filter(|b| b.kind != BodyKind::Star && b.id != from)
        .filter(|b| g.bodies[b.id.0 as usize].owner != Some(me))
        .filter(|b| !g.players[me as usize].bot_avoid.iter().any(|a| a.0 == from && a.1 == b.id))
        // don't send a second squad where one is already heading
        .filter(|b| {
            !g.ships.iter().any(|s| {
                s.owner == me
                    && match &s.state {
                        ShipState::Flying(f) => f.dest == Some(b.id),
                        ShipState::Parked { departure: Some(dep), .. } => dep.to == b.id,
                        _ => false,
                    }
            })
        })
        .min_by(|a, b| {
            let score = |x: &crate::system::Body| {
                let dist = g.system.pos(x.id, g.t).dist(here);
                let enemy = g.bodies[x.id.0 as usize].owner.is_some();
                let value = match x.kind {
                    BodyKind::Planet | BodyKind::GasGiant => 3.0,
                    _ => 1.0,
                };
                // hard bots go for enemy holdings, easy ones avoid fights
                let fight = match (d, enemy) {
                    (Difficulty::Hard, true) => 0.6,
                    (Difficulty::Easy, true) => 3.0,
                    (_, true) => 1.5,
                    _ => 1.0,
                };
                dist * fight / value
            };
            score(a).total_cmp(&score(b))
        })
        .map(|b| b.id)
}

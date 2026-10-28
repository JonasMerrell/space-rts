//! One log line per fleet: ship events of the same tick with the same title, place and text
//! (arrivals, departures, losses …) merge into a single line, e.g. "Fleet Alpha (5 ships) is in
//! orbit of Kepler-3" or "3 ships were destroyed at Ixvo".
use crate::fleet::FleetId;
use crate::game::{Event, EventKind, Game, PlayerId, ShipId};
use crate::BodyId;

/// The ships behind a merged log line. Only lives for the tick it was written in (not sent or saved).
#[derive(Clone, Debug, Default)]
pub struct ShipGroup {
    /// (ship, ship name, (fleet, fleet name, fleet size) when in a fleet)
    ships: Vec<(ShipId, String, Option<(FleetId, String, usize)>)>,
    title: (String, String),
    verb: (String, String),
    rest: String,
}

fn fleet_label(name: &str) -> String {
    if name.to_lowercase().starts_with("fleet") {
        name.to_string()
    } else {
        format!("Fleet {name}")
    }
}

fn ships(n: usize) -> String {
    if n == 1 {
        "1 ship".into()
    } else {
        format!("{n} ships")
    }
}

impl ShipGroup {
    /// (title, body) for the whole group.
    fn render(&self) -> (String, String) {
        // (subject, is plural)
        let mut parts: Vec<(String, bool)> = vec![];
        let mut fleets: Vec<FleetId> = vec![];
        for (_, _, f) in &self.ships {
            if let Some((id, ..)) = f {
                if !fleets.contains(id) {
                    fleets.push(*id);
                }
            }
        }
        for id in fleets {
            let members: Vec<_> = self
                .ships
                .iter()
                .filter(|s| s.2.as_ref().is_some_and(|f| f.0 == id))
                .collect();
            let (_, fname, size) = members[0].2.as_ref().unwrap();
            let k = members.len();
            parts.push(match k {
                1 => (members[0].1.clone(), false),
                _ if k >= *size => (format!("{} ({})", fleet_label(fname), ships(k)), false),
                _ => (format!("{} of {}", ships(k), fleet_label(fname)), true),
            });
        }
        let loose: Vec<_> = self.ships.iter().filter(|s| s.2.is_none()).collect();
        match loose.len() {
            0 => {}
            1 => parts.push((loose[0].1.clone(), false)),
            k if parts.is_empty() => parts.push((ships(k), true)),
            k => parts.push((format!("{k} other ships"), true)),
        }
        let plural = parts.len() > 1 || parts[0].1;
        let names: Vec<String> = parts.into_iter().map(|p| p.0).collect();
        let subject = match names.len() {
            1 => names[0].clone(),
            n => format!("{} and {}", names[..n - 1].join(", "), names[n - 1]),
        };
        let verb = if plural { &self.verb.1 } else { &self.verb.0 };
        let title = if self.ships.len() > 1 {
            &self.title.1
        } else {
            &self.title.0
        };
        (
            title.clone(),
            [subject.as_str(), verb, &self.rest]
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(" "),
        )
    }
}

/// A log line about one ship: "{ship} {verb} {rest}", with singular/plural forms of the title and verb.
pub(crate) struct ShipNote<'a> {
    pub to: PlayerId,
    pub kind: EventKind,
    pub title: (&'a str, &'a str),
    pub ship: ShipId,
    pub name: &'a str,
    pub verb: (&'a str, &'a str),
    pub rest: String,
    pub body_ref: Option<BodyId>,
    pub ship_ref: Option<ShipId>,
}

impl Game {
    /// Log a ship event, merged into a matching line from the same tick when there is one.
    /// Call before the ship leaves its fleet (destroyed ships are still listed until `fleet_tick`).
    pub(crate) fn ship_note(&mut self, n: ShipNote) {
        let fleet = self
            .fleet_of(n.ship)
            .map(|f| (f.id, f.name.clone(), f.ships.len()));
        let member = (n.ship, n.name.to_string(), fleet);
        let t = self.t;
        let same = |e: &Event| {
            e.to == Some(n.to)
                && e.kind == n.kind
                && e.body_ref == n.body_ref
                && e.group.as_ref().is_some_and(|g| {
                    g.title.0 == n.title.0 && g.verb.0 == n.verb.0 && g.rest == n.rest
                })
        };
        if let Some(e) = self
            .events
            .iter_mut()
            .rev()
            .take_while(|e| e.t == t)
            .find(|e| same(e))
        {
            let g = e.group.as_mut().unwrap();
            if !g.ships.iter().any(|s| s.0 == n.ship) {
                g.ships.push(member);
            }
            let (title, body) = g.render();
            e.title = title;
            e.body = body;
            e.ship_ref = e.ship_ref.or(n.ship_ref);
            return;
        }
        let g = ShipGroup {
            ships: vec![member],
            title: (n.title.0.into(), n.title.1.into()),
            verb: (n.verb.0.into(), n.verb.1.into()),
            rest: n.rest,
        };
        let (title, body) = g.render();
        self.event(Some(n.to), n.kind, &title, &body, n.body_ref, n.ship_ref);
        self.events.last_mut().unwrap().group = Some(g);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(ships: &[(&str, Option<(u32, &str, usize)>)]) -> ShipGroup {
        ShipGroup {
            ships: ships
                .iter()
                .enumerate()
                .map(|(i, (n, f))| {
                    (
                        ShipId(i as u32),
                        n.to_string(),
                        f.map(|(id, fname, k)| (FleetId(id), fname.to_string(), k)),
                    )
                })
                .collect(),
            title: ("Ship destroyed".into(), "Ships destroyed".into()),
            verb: ("was".into(), "were".into()),
            rest: "destroyed at Ixvo".into(),
        }
    }

    #[test]
    fn subjects() {
        let r = |s: &[(&str, Option<(u32, &str, usize)>)]| group(s).render();
        assert_eq!(
            r(&[("Vega", None)]),
            ("Ship destroyed".into(), "Vega was destroyed at Ixvo".into())
        );
        assert_eq!(
            r(&[("Vega", None), ("Rigel", None), ("Deneb", None)]).1,
            "3 ships were destroyed at Ixvo"
        );
        let a = Some((0, "Alpha", 2));
        assert_eq!(
            r(&[("Vega", a), ("Rigel", a)]),
            (
                "Ships destroyed".into(),
                "Fleet Alpha (2 ships) was destroyed at Ixvo".into()
            )
        );
        let b = Some((1, "Fleet 2", 5));
        assert_eq!(
            r(&[("Vega", b), ("Rigel", b), ("Deneb", None)]).1,
            "2 ships of Fleet 2 and Deneb were destroyed at Ixvo"
        );
        assert_eq!(
            r(&[("Vega", a), ("Rigel", a), ("Deneb", None), ("Sol", None)]).1,
            "Fleet Alpha (2 ships) and 2 other ships were destroyed at Ixvo"
        );
    }
}

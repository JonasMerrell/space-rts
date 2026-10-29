//! Property tests: whatever a client sends, the server neither panics nor stops answering.
//! Messages are generated as JSON (valid shapes with hostile values, and random mutations of
//! them), decoded the way `client_thread` does, then run through `Server::handle` against a
//! running game with bots, while the game keeps advancing. `FUZZ_CASES=n` for a longer run.
use super::*;
use proptest::prelude::*;
use serde_json::{json, Value};
use std::sync::mpsc::Receiver;

fn cases() -> u32 {
    std::env::var("FUZZ_CASES").ok().and_then(|s| s.parse().ok()).unwrap_or(16)
}

fn num() -> impl Strategy<Value = Value> {
    prop_oneof![
        (-1e4f64..1e6).prop_map(|x| json!(x)),
        Just(json!(0)),
        Just(json!(-1)),
        Just(json!(1e308)),
        Just(json!(-1e308)),
        Just(json!(5e-324)),
        Just(json!(u64::MAX)),
        Just(json!(i64::MIN)),
        any::<f64>().prop_filter("json numbers are finite", |x| x.is_finite()).prop_map(|x| json!(x)),
    ]
}

fn id() -> impl Strategy<Value = Value> {
    prop_oneof![4 => 0u32..48, 1 => Just(u32::MAX), 1 => any::<u32>()].prop_map(|x| json!(x))
}

fn ids() -> impl Strategy<Value = Value> {
    prop::collection::vec(id(), 0..6).prop_map(Value::from)
}

fn text() -> impl Strategy<Value = Value> {
    prop_oneof![
        "[a-zA-Z0-9 ]{0,12}".prop_map(Value::from),
        any::<String>().prop_map(Value::from),
        Just(json!("x".repeat(5000))),
        Just(json!("\u{202e}\u{0}\u{feff}Bot 1 (normal)")),
    ]
}

fn pick(names: &'static [&'static str]) -> impl Strategy<Value = Value> {
    prop::sample::select(names).prop_map(Value::from)
}

const CLASSES: &[&str] = &["Scout", "Interceptor", "Gunship", "Bomber", "Dreadnought", "Tanker", "Constructor", "Nope"];
const UPGRADES: &[&str] = &["Mining", "Solar", "Reactor", "Refinery", "Shipyard", "Defense", "Telescope", "Research"];
const TECHS: &[&str] = &["HardenedSlugs", "IonDrives", "AsteroidDrives", "ModularYards", "DeepSpaceArray", "SignatureDamping"];
const STATIONS: &[&str] = &["Shipyard", "FuelDepot", "Telescope", "Scanner", "Defense", "Relay"];

fn burn() -> impl Strategy<Value = Value> {
    (num(), num(), num()).prop_map(|(t, p, r)| json!({"t": t, "prograde": p, "radial": r}))
}

fn hint() -> impl Strategy<Value = Value> {
    prop_oneof![
        num().prop_map(|tof| json!({"Direct": {"tof": tof}})),
        (id(), num(), num()).prop_map(|(via, f, a)| json!({"Assist": {"via": via, "flyby_t": f, "arrive_t": a}})),
    ]
}

fn opt<S: Strategy<Value = Value>>(s: S) -> impl Strategy<Value = Value> {
    prop_oneof![Just(Value::Null), s]
}

fn fleet_cmd() -> impl Strategy<Value = Value> {
    prop_oneof![
        (ids(), text()).prop_map(|(s, n)| json!({"Form": {"ships": s, "name": n}})),
        (id(), ids()).prop_map(|(f, s)| json!({"Join": {"fleet": f, "ships": s}})),
        (ids(), opt(text())).prop_map(|(s, n)| json!({"Leave": {"ships": s, "split": n}})),
        (id(), id()).prop_map(|(a, b)| json!({"Merge": {"into": a, "from": b}})),
        (id(), text()).prop_map(|(f, n)| json!({"Rename": {"fleet": f, "name": n}})),
        id().prop_map(|f| json!({"Disband": {"fleet": f}})),
    ]
}

fn command() -> impl Strategy<Value = Value> {
    prop_oneof![
        (ids(), id(), opt(num())).prop_map(|(s, to, d)| json!({"Transfer": {"ships": s, "to": to, "depart_t": d}})),
        (ids(), id(), num(), hint()).prop_map(|(s, to, d, h)| json!({"TransferPlanned": {"ships": s, "to": to, "depart_t": d, "hint": h}})),
        id().prop_map(|s| json!({"CancelDeparture": {"ship": s}})),
        (id(), prop::collection::vec(burn(), 0..4)).prop_map(|(s, b)| json!({"SetBurns": {"ship": s, "burns": b}})),
        (id(), pick(CLASSES)).prop_map(|(b, c)| json!({"Build": {"body": b, "class": c}})),
        (id(), pick(UPGRADES)).prop_map(|(b, u)| json!({"Upgrade": {"body": b, "upgrade": u}})),
        (id(), pick(&["Hold", "Retreat"])).prop_map(|(s, o)| json!({"SetOrder": {"ship": s, "order": o}})),
        (id(), pick(TECHS)).prop_map(|(b, t)| json!({"Research": {"body": b, "tech": t}})),
        (id(), pick(STATIONS)).prop_map(|(b, k)| json!({"BuildStation": {"body": b, "kind": k}})),
        text().prop_map(|t| json!({"Chat": {"text": t}})),
        num().prop_map(|s| json!({"Extend": {"seconds": s}})),
        fleet_cmd().prop_map(|f| json!({"Fleet": f})),
        (id(), burn()).prop_map(|(b, u)| json!({"PushAsteroid": {"body": b, "burn": u}})),
        (id(), id()).prop_map(|(b, t)| json!({"AimAsteroid": {"body": b, "target": t}})),
        (id(), num(), opt(id()), opt(num())).prop_map(|(b, d, to, dt)| json!({"QueueOrder": {"body": b, "done_t": d, "to": to, "depart_t": dt}})),
        (id(), num()).prop_map(|(b, i)| json!({"CancelJob": {"body": b, "index": i}})),
        (id(), num(), num()).prop_map(|(b, f, t)| json!({"MoveJob": {"body": b, "from": f, "to": t}})),
        num().prop_map(|s| json!({"SetSpeed": {"speed": s}})),
    ]
}

/// A message for game `code` (or a garbage code).
fn client_msg(code: &'static str) -> impl Strategy<Value = Value> {
    let code = || prop_oneof![4 => Just(json!(code)), 1 => text()];
    prop_oneof![
        8 => (code(), command()).prop_map(|(c, cmd)| json!({"Cmd": {"code": c, "cmd": cmd}})),
        1 => Just(json!("ListGames")),
        1 => Just(json!("Ping")),
        1 => num().prop_map(|s| json!({"Events": {"since": s}})),
        1 => code().prop_map(|c| json!({"Open": {"code": c}})),
        1 => code().prop_map(|c| json!({"JoinGame": {"code": c}})),
        1 => code().prop_map(|c| json!({"StartGame": {"code": c}})),
        1 => (code(), text()).prop_map(|(c, d)| json!({"AddBot": {"code": c, "difficulty": d}})),
        1 => (code(), id()).prop_map(|(c, p)| json!({"RemoveBot": {"code": c, "player": p}})),
        1 => (code(), text()).prop_map(|(c, f)| json!({"SetFaction": {"code": c, "faction": f}})),
        1 => (text(), pick(&["Small", "Medium", "Large", "Huge"]), num(), num(), num(), prop::collection::vec(text(), 0..10))
            .prop_map(|(n, sz, l, sp, m, b)| json!({"CreateGame": {"name": n, "size": sz, "length_days": l, "speed": sp, "max_players": m, "bots": b}})),
        1 => (text(), text(), any::<bool>()).prop_map(|(u, p, r)| json!({"Hello": {"user": u, "pass": p, "register": r}})),
    ]
}

/// Damage a JSON value in one random place.
fn mutate(v: &mut Value, mut seed: u64, depth: u32) {
    let mut next = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        seed >> 33
    };
    match v {
        Value::Object(m) if !m.is_empty() && depth < 8 => {
            let k = m.keys().nth(next() as usize % m.len()).cloned().unwrap_or_default();
            match next() % 4 {
                0 => {
                    m.remove(&k);
                }
                1 => {
                    m.insert(format!("{k}_extra"), json!([1, {"a": null}]));
                }
                _ => {
                    if let Some(x) = m.get_mut(&k) {
                        mutate(x, next(), depth + 1);
                    }
                }
            }
        }
        Value::Array(a) if !a.is_empty() && depth < 8 => {
            let i = next() as usize % a.len();
            mutate(&mut a[i], next(), depth + 1);
        }
        _ => {
            *v = match next() % 7 {
                0 => Value::Null,
                1 => json!(-1),
                2 => json!(1e300),
                3 => json!("str"),
                4 => json!([]),
                5 => json!({}),
                _ => json!(true),
            }
        }
    }
}

struct World {
    s: Server,
    _rx: Vec<Receiver<String>>,
    code: String,
}

/// A server with "ann" (host, conn 1) + "bob" (conn 2) in a running game with two bots.
fn world() -> World {
    static TEMPLATE: std::sync::OnceLock<Game> = std::sync::OnceLock::new();
    let dir = std::env::temp_dir().join(format!("server-fuzz-{}", std::process::id()));
    let mut s = Server::load(dir); // nothing is saved: the tests never call save()
    let mut rx = vec![];
    for (id, user) in [(1u64, "ann"), (2, "bob")] {
        let (tx, r) = sync_channel(1 << 16);
        rx.push(r);
        s.conns.insert(id, Conn { user: Some(user.into()), open: None, tx, sock: None, ip: "203.0.113.9".parse().unwrap() });
    }
    let g = TEMPLATE.get_or_init(|| {
        s.handle(1, ClientMsg::CreateGame { name: "fuzz".into(), size: SystemSize::Small, length_days: 7.0, speed: 100.0, max_players: 4, bots: vec!["normal".into(), "hard".into()], teams: 0 });
        let code = s.games.keys().next().cloned().unwrap();
        s.handle(2, ClientMsg::JoinGame { code: code.clone() });
        s.handle(1, ClientMsg::StartGame { code: code.clone() });
        let mut g = s.games.remove(&code).unwrap();
        let t = g.t + 3600.0;
        g.advance_to(t);
        g
    });
    let code = g.code.clone();
    s.games.insert(code.clone(), g.clone());
    World { s, _rx: rx, code }
}

use sim::system::SystemSize;

/// Decode like `client_thread`, then handle; returns whether the message got through decoding.
fn feed(w: &mut World, conn: u64, line: &str) -> bool {
    match decode::<ClientMsg>(line).and_then(|m| m.check().map(|_| m).map_err(String::from)) {
        Ok(ClientMsg::Hello { .. }) => true, // `hello` needs a socket; covered by the e2e tests
        Ok(m) => {
            let t = std::time::Instant::now();
            let name = format!("{m:?}").chars().take(60).collect::<String>();
            w.s.handle(conn, m);
            if t.elapsed().as_millis() > 100 && std::env::var("FUZZ_TIMING").is_ok() {
                eprintln!("SLOW {:?} {name}", t.elapsed());
            }
            true
        }
        Err(_) => false,
    }
}

fn step(w: &mut World, dt: f64) {
    if let Some(g) = w.s.games.get_mut(&w.code) {
        let t = (g.t + dt).min(g.end_t);
        g.advance_to(t);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(), failure_persistence: None, ..ProptestConfig::default() })]

    /// Well-formed messages with hostile values, from the host and a player, while time passes.
    #[test]
    fn hostile_messages_never_panic(msgs in prop::collection::vec((client_msg("CODE"), any::<bool>(), 0.0f64..1800.0), 1..16)) {
        let mut w = world();
        for (m, host, dt) in msgs {
            let line = m.to_string().replace("\"CODE\"", &format!("\"{}\"", w.code));
            feed(&mut w, if host { 1 } else { 2 }, &line);
            step(&mut w, dt);
        }
        // still answering
        w.s.handle(1, ClientMsg::ListGames);
        prop_assert!(w.s.games.contains_key(&w.code));
    }

    /// Valid messages damaged in random places: must be rejected or handled, never panic.
    #[test]
    fn mutated_messages_never_panic(msgs in prop::collection::vec((client_msg("CODE"), any::<u64>()), 1..16)) {
        let mut w = world();
        for (mut m, seed) in msgs {
            mutate(&mut m, seed, 0);
            let line = m.to_string().replace("\"CODE\"", &format!("\"{}\"", w.code));
            feed(&mut w, 2, &line);
            step(&mut w, 300.0);
        }
    }

    /// Arbitrary bytes and text never panic the decoder.
    #[test]
    fn garbage_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..512), s in ".{0,300}") {
        let _ = decode::<ClientMsg>(&String::from_utf8_lossy(&bytes));
        let _ = decode::<ClientMsg>(&s);
        let mut r = std::io::BufReader::new(&bytes[..]);
        while let Ok(Some(_)) = wire::read_line(&mut r, 64) {}
    }
}

#[test]
fn deep_nesting_is_rejected_not_a_stack_overflow() {
    let deep = "[".repeat(100_000) + &"]".repeat(100_000);
    assert!(decode::<ClientMsg>(&format!("{{\"Cmd\":{{\"code\":\"X\",\"cmd\":{{\"Fleet\":{deep}}}}}}}")).is_err());
    let deep = "{\"a\":".repeat(50_000) + "1" + &"}".repeat(50_000);
    assert!(decode::<ClientMsg>(&deep).is_err());
}


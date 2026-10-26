//! Full stack without a window: 4 real `ClientBackend`s (the phone app's network + local sim layer)
//! against a real in-process server over TCP. Checks what the UI sees through `ui::Backend`.
use space_rts::backend::ClientBackend;
use std::net::TcpListener;
use std::time::{Duration, Instant};
use ui::model::BodyKind;
use ui::{Backend, BackendEvent, PlanPoll, UiAction};

fn start_server() -> String {
    let dir = std::env::temp_dir().join(format!("simtests-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    std::thread::spawn(move || server::serve(l, dir));
    addr
}

/// Poll `be` (like the UI does every frame) until `f` returns Some.
fn until<T>(be: &mut ClientBackend, what: &str, secs: f64, mut f: impl FnMut(&mut ClientBackend, &[BackendEvent]) -> Option<T>) -> T {
    let end = Instant::now() + Duration::from_secs_f64(secs);
    let mut seen = vec![];
    while Instant::now() < end {
        let evs = be.poll();
        seen.extend(evs.iter().map(|e| format!("{e:?}")));
        if let Some(x) = f(be, &evs) {
            return x;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}; events seen: {seen:?}");
}

#[test]
fn four_phones_play_a_game_through_the_real_client_backend() {
    let addr = start_server();
    let names = ["ann", "bob", "cat", "dan"];
    let mut cs: Vec<ClientBackend> = names.iter().map(|_| ClientBackend::new(addr.clone())).collect();

    // register + log in
    for (c, n) in cs.iter_mut().zip(names) {
        c.act(UiAction::Login { server: addr.clone(), user: n.into(), pass: "pw".into(), register: true });
        until(c, "LoggedIn", 10.0, |_, ev| ev.iter().any(|e| matches!(e, BackendEvent::LoggedIn { user } if user == n)).then_some(()));
    }
    // wrong password is reported as LoginFailed
    let mut bad = ClientBackend::new(addr.clone());
    bad.act(UiAction::Login { server: addr.clone(), user: "ann".into(), pass: "nope".into(), register: false });
    until(&mut bad, "LoginFailed", 10.0, |_, ev| ev.iter().any(|e| matches!(e, BackendEvent::LoginFailed(_))).then_some(()));

    // ann hosts; the lobby event carries the join code
    cs[0].act(UiAction::CreateGame { name: "E2E".into(), length_days: 7, speed: 1.0, bots: vec![] });
    let code = until(&mut cs[0], "Lobby", 10.0, |_, ev| {
        ev.iter().find_map(|e| if let BackendEvent::Lobby(l) = e { Some(l.code.clone()) } else { None })
    });
    for c in cs.iter_mut().skip(1) {
        c.act(UiAction::JoinGame { code: code.to_lowercase() });
        until(c, "joined lobby", 10.0, |_, ev| ev.iter().any(|e| matches!(e, BackendEvent::Lobby(l) if l.code == code)).then_some(()));
    }
    let n = until(&mut cs[0], "4 in lobby", 30.0, |_, ev| {
        ev.iter().find_map(|e| if let BackendEvent::Lobby(l) = e { (l.players.len() == 4).then_some(l.max_players) } else { None })
    });
    assert!(n >= 4);
    assert!(cs[0].my_games().iter().any(|g| g.code == code) || cs[0].view().is_none());

    // start: everybody gets GameStarted and a view where `me` is themselves
    cs[0].act(UiAction::StartGame);
    let mut mes = vec![];
    for (c, n) in cs.iter_mut().zip(names) {
        until(c, "GameStarted", 30.0, |b, ev| (ev.iter().any(|e| matches!(e, BackendEvent::GameStarted)) || b.view().is_some_and(|v| !v.ships.is_empty())).then_some(()));
        let v = c.view().expect("view");
        let me = v.player(v.me).expect("me in players");
        assert_eq!(me.name, n);
        mes.push(v.me);
        assert!(v.bodies.iter().any(|b| b.kind == BodyKind::Star));
        assert!(v.ends_at > v.now);
    }
    mes.sort();
    mes.dedup();
    assert_eq!(mes.len(), 4);

    // every phone predicts the same body positions (same deterministic sim)
    let v0 = cs[0].view().unwrap().clone();
    let t = v0.now + 7200.0;
    for b in &v0.bodies {
        let p0 = cs[0].body_pos(b.id, t);
        for c in &cs[1..] {
            let p = c.body_pos(b.id, t);
            assert!((p[0] - p0[0]).abs() < 1e-6 && (p[1] - p0[1]).abs() < 1e-6, "{} differs between clients", b.name);
        }
    }

    // async planner: returns immediately, finishes in the background, matches the sync answer
    let me = v0.me;
    let ship = v0.ships.iter().find(|s| s.owner == me && s.parked_at.is_some() && s.fuel > 0.0).expect("parked ship").clone();
    // nearest-ish other planet (owners of far bodies may be hidden by fog of war)
    let target = v0.bodies.iter().find(|b| b.kind == BodyKind::Planet && Some(b.id) != ship.parked_at && b.parent == Some(0)).expect("another planet").id;
    let t0 = Instant::now();
    let first = cs[0].plan_transfer_async(ship.id, target, v0.now);
    assert!(t0.elapsed() < Duration::from_millis(100), "plan_transfer_async blocked for {:?}", t0.elapsed());
    let plan = match first {
        PlanPoll::Done(p) => p,
        PlanPoll::Pending => until(&mut cs[0], "async plan", 120.0, |b, _| match b.plan_transfer_async(ship.id, target, v0.now) {
            PlanPoll::Done(p) => Some(p),
            PlanPoll::Pending => None,
        }),
    };
    let plan = plan.expect("a transfer to another planet");
    let sync = cs[0].plan_transfer(ship.id, target, v0.now).expect("sync plan");
    assert_eq!(plan.depart_t, sync.depart_t);
    assert_eq!(plan.arrive_t, sync.arrive_t);
    // the system is random per server game: some first targets cost more than a full tank (the UI warns);
    // departure still works, so the rest of the flow is valid either way
    if plan.fuel_needed > ship.fuel {
        eprintln!("note: plan needs {} fuel, ship has {}", plan.fuel_needed, ship.fuel);
    }

    // schedule it; the server accepts it and another phone sees the departure order
    cs[0].act(UiAction::ScheduleTransfer(plan.clone()));
    until(&mut cs[0], "own ship has a destination", 30.0, |b, _| b.view()?.ship(ship.id)?.destination.map(|_| ()));
    // and it survives a reconnect: a fresh client logged in as ann sees the same order
    let mut ann2 = ClientBackend::new(addr.clone());
    ann2.act(UiAction::Login { server: addr.clone(), user: "ann".into(), pass: "pw".into(), register: false });
    until(&mut ann2, "ann re-login", 10.0, |_, ev| ev.iter().any(|e| matches!(e, BackendEvent::LoggedIn { .. })).then_some(()));
    ann2.act(UiAction::OpenGame { code: code.clone() });
    until(&mut ann2, "ann2 sees her ship's order", 30.0, |b, _| b.view()?.ship(ship.id)?.destination.map(|_| ()));

    // chat goes everywhere
    cs[2].act(UiAction::Chat("gl hf".into()));
    for c in cs.iter_mut() {
        until(c, "chat", 30.0, |b, _| b.view()?.chat.iter().any(|m| m.text == "gl hf").then_some(()));
    }

    // a rule violation comes back as a notice, not a disconnect
    cs[1].act(UiAction::SetOrder { ship: ship.id, retreat: true });
    until(&mut cs[1], "rejection notice", 30.0, |_, ev| ev.iter().any(|e| matches!(e, BackendEvent::Notice(_))).then_some(()));
    cs[1].act(UiAction::Chat("still here".into()));
    until(&mut cs[0], "bob still connected", 30.0, |b, _| b.view()?.chat.iter().any(|m| m.text == "still here").then_some(()));
}

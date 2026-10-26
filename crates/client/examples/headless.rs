//! Drive ClientBackend against a live server without a window: login, create, start, plan, fly.
use space_rts::backend::ClientBackend;
use ui::{Backend, BackendEvent, UiAction};
use std::time::{Duration, Instant};

fn pump(be: &mut ClientBackend, secs: f64) -> Vec<BackendEvent> {
    let t0 = Instant::now();
    let mut all = vec![];
    while t0.elapsed().as_secs_f64() < secs {
        all.extend(be.poll());
        std::thread::sleep(Duration::from_millis(50));
    }
    all
}

fn main() {
    let server = std::env::args().nth(1).unwrap_or("127.0.0.1:7777".into());
    let mut be = ClientBackend::new(server);
    be.act(UiAction::Login { server: String::new(), user: "headless".into(), pass: "pw".into(), register: true });
    for e in pump(&mut be, 1.0) { println!("{e:?}"); }
    be.act(UiAction::CreateGame { name: "headless test".into(), length_days: 7, bots: vec!["normal".into()] });
    for e in pump(&mut be, 1.0) { println!("{}", format!("{e:?}").chars().take(200).collect::<String>()); }
    be.act(UiAction::StartGame);
    for e in pump(&mut be, 1.5) { println!("{}", format!("{e:?}").chars().take(200).collect::<String>()); }
    let v = be.view().expect("view").clone();
    println!("view: {} bodies, {} ships, now {:.0}, res {:?}", v.bodies.len(), v.ships.len(), v.now, v.resources);
    let mine = v.ships.iter().find(|s| s.owner == v.me && s.parked_at.is_some()).unwrap();
    let home = mine.parked_at.unwrap();
    let target = v.bodies.iter().filter(|b| b.kind == ui::model::BodyKind::Planet && b.id != home).min_by(|a, b| {
        let d = |x: &ui::model::Body| { let p = be.body_pos(x.id, v.now); let h = be.body_pos(home, v.now); (p[0]-h[0]).hypot(p[1]-h[1]) };
        d(a).total_cmp(&d(b))
    }).unwrap();
    let t0 = Instant::now();
    let plan = be.plan_transfer(mine.id, target.id, v.now).expect("plan");
    println!("plan {} -> {} in {:?}: depart +{:.1}h arrive +{:.1}h dv {:.2}+{:.2} fuel {:.0}/{:.0} windows {}", mine.name, target.name, t0.elapsed(),
        (plan.depart_t - v.now)/3600.0, (plan.arrive_t - v.now)/3600.0, plan.dv_depart, plan.dv_capture, plan.fuel_needed, mine.fuel, plan.windows.len());
    be.act(UiAction::ScheduleTransfer(plan.clone()));
    for e in pump(&mut be, 1.5) { println!("{}", format!("{e:?}").chars().take(200).collect::<String>()); }
    let v = be.view().unwrap();
    let s = v.ship(mine.id).unwrap();
    println!("after schedule: dest {:?}, path pts {}", s.destination, s.trajectory.as_ref().map(|t| t.points.len()).unwrap_or(0));
    println!("ship pos at arrival {:?} vs target {:?}", be.ship_pos(mine.id, plan.arrive_t), be.body_pos(target.id, plan.arrive_t));
    be.act(UiAction::Chat("hello from headless".into()));
    pump(&mut be, 1.0);
    println!("chat: {:?}", be.view().unwrap().chat.last().map(|c| c.text.clone()));
}

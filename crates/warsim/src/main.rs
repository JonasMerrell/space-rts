//! warsim: run whole bot-vs-bot wars headless and fast, for balance tuning.
//!
//!   warsim run    [options]        play many wars, one JSON line per war
//!   warsim duel   [options]        equal-cost fleets of each ship class fight each other
//!   warsim report <file.jsonl>...  summary tables from `run` output
//!
//! Every war uses the real `sim::Game` with the server's bots in every slot, so results follow the
//! current rules (`sim::game::RULES`). Wars are deterministic per seed.
use serde::{Deserialize, Serialize};
use sim::game::*;
use sim::{BodyId, BodyKind, SystemSize};
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

const HOUR: f64 = 3600.0;
const DAY: f64 = 86_400.0;
/// how often the war is sampled for ship builds/losses (game seconds)
const SAMPLE: f64 = 600.0;
/// score timeline resolution
const TIMELINE: f64 = 12.0 * HOUR;

fn usage() -> ! {
    eprintln!(
        "warsim — headless bot wars for balance tuning

warsim run [--games N] [--players 2,4,8] [--sizes small,medium,large] [--bots hard|normal|easy|mixed|hard,normal...]
           [--teams ffa|teams|both] [--days 14] [--seed 1] [--threads N] [--out wars.jsonl]
    Plays N wars per (players, size, teams) combination. --bots picks the difficulty of every slot:
    one name for all, a comma list cycled over the slots, or 'mixed' (slot difficulties drawn per war).
    --teams teams uses 2 teams. Appends one JSON line per war to --out (default stdout).

warsim duel [--budget 3000] [--classes interceptor,gunship,bomber,dreadnought] [--reps 5] [--scale dreadnought=0.5]
    Every pair of classes fights at a neutral moon with fleets of about equal matter+energy cost.
    --scale prices a class differently when sizing fleets, to try out a new price.

warsim report FILE...
    Win rates by home orbit and kind, snowballing, ship use and losses, economy pace, game length."
    );
    std::process::exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else { usage() };
    let opts = Opts::parse(&args[1..]);
    match cmd.as_str() {
        "run" => run(&opts),
        "duel" => duel(&opts),
        "report" => report(&opts.files),
        _ => usage(),
    }
}

struct Opts {
    kv: HashMap<String, String>,
    files: Vec<String>,
}

impl Opts {
    fn parse(a: &[String]) -> Opts {
        let mut kv = HashMap::new();
        let mut files = vec![];
        let mut i = 0;
        while i < a.len() {
            if let Some(k) = a[i].strip_prefix("--") {
                let v = a.get(i + 1).cloned().unwrap_or_else(|| usage());
                kv.insert(k.to_string(), v);
                i += 2;
            } else {
                files.push(a[i].clone());
                i += 1;
            }
        }
        Opts { kv, files }
    }
    fn get(&self, k: &str, d: &str) -> String {
        self.kv.get(k).cloned().unwrap_or_else(|| d.to_string())
    }
    fn num(&self, k: &str, d: f64) -> f64 {
        self.get(k, &d.to_string()).parse().unwrap_or_else(|_| usage())
    }
    fn list(&self, k: &str, d: &str) -> Vec<String> {
        self.get(k, d).split(',').map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()).collect()
    }
}

fn parse_size(s: &str) -> SystemSize {
    match s {
        "small" => SystemSize::Small,
        "large" => SystemSize::Large,
        _ => SystemSize::Medium,
    }
}

fn parse_class(s: &str) -> ShipClass {
    *ShipClass::ALL.iter().find(|c| c.stats().name.eq_ignore_ascii_case(s)).unwrap_or_else(|| usage())
}

// ---------------------------------------------------------------------------------------------
// run

#[derive(Clone)]
struct Job {
    seed: u64,
    players: usize,
    size: String,
    teams: usize,
    bots: Vec<String>,
    days: f64,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct PlayerOut {
    id: u32,
    bot: String,
    team: u8,
    /// "rocky" or "giant"
    home_kind: String,
    /// 0 = innermost planet of the system
    home_rank: usize,
    /// home_rank scaled to 0 (innermost) .. 1 (outermost planet)
    home_pos: f64,
    score: u32,
    bodies: usize,
    planets: usize,
    rank: usize,
    won: bool,
    kept_home: bool,
    /// game day the player first held a second body
    first_capture_day: Option<f64>,
    /// game day the player held nothing any more
    out_day: Option<f64>,
    /// bodies taken from nobody / from an enemy, and bodies lost to anyone
    #[serde(default)]
    took_neutral: u32,
    #[serde(default)]
    took_enemy: u32,
    #[serde(default)]
    lost_bodies: u32,
    built: BTreeMap<String, u32>,
    lost: BTreeMap<String, u32>,
    peak_ships: usize,
    final_ships: usize,
    techs: usize,
    stations: usize,
    /// matter per hour at the end of day 1, 3, 7 and the end
    matter_rate: Vec<f64>,
    energy_rate: Vec<f64>,
    /// matter and energy banked (unspent) at the same checkpoints
    #[serde(default)]
    matter_bank: Vec<f64>,
    #[serde(default)]
    energy_bank: Vec<f64>,
    /// score at every 12 h
    timeline: Vec<u32>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct WarOut {
    rules: u32,
    seed: u64,
    size: String,
    players: usize,
    teams: usize,
    days: f64,
    bodies: usize,
    planets: usize,
    winner: Option<u32>,
    winning_team: Option<u8>,
    /// top score minus second (sides in team games)
    margin: i64,
    /// changes of the leading side between 12 h samples
    lead_changes: u32,
    /// first day from which the eventual winner led at every sample
    locked_day: f64,
    neutral_left: usize,
    battles: u32,
    wall_s: f64,
    pilots: Vec<PlayerOut>,
}

fn run(o: &Opts) {
    let games = o.num("games", 10.0) as usize;
    let players: Vec<usize> = o.list("players", "4").iter().map(|s| s.parse().unwrap_or_else(|_| usage())).collect();
    let sizes = o.list("sizes", "medium");
    let teams_opt = o.get("teams", "ffa");
    let bots = o.list("bots", "hard");
    let days = o.num("days", 14.0);
    let seed0 = o.num("seed", 1.0) as u64;
    let threads = o.num("threads", std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4) as f64) as usize;
    let mut jobs = vec![];
    for &n in &players {
        for size in &sizes {
            let team_modes: Vec<usize> = match teams_opt.as_str() {
                "teams" => vec![2],
                "both" => vec![0, 2],
                _ => vec![0],
            };
            for teams in team_modes {
                if teams > 0 && (n < 4 || n % teams != 0) {
                    continue;
                }
                for g in 0..games {
                    let seed = seed0 + g as u64;
                    let slot_bots = (0..n)
                        .map(|i| {
                            if bots == ["mixed"] {
                                let mut r = sim::system::Rng::new(seed * 31 + i as u64 * 7919 + n as u64);
                                ["easy", "normal", "hard"][r.below(3) as usize].to_string()
                            } else {
                                bots[i % bots.len()].clone()
                            }
                        })
                        .collect();
                    jobs.push(Job { seed, players: n, size: size.clone(), teams, bots: slot_bots, days });
                }
            }
        }
    }
    let out: Mutex<Box<dyn Write + Send>> = Mutex::new(match o.kv.get("out") {
        Some(p) => Box::new(std::fs::OpenOptions::new().create(true).append(true).open(p).expect("open --out")),
        None => Box::new(std::io::stdout()),
    });
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let total = jobs.len();
    eprintln!("warsim: {total} wars on {threads} threads, rules {RULES}");
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(job) = jobs.get(i) else { break };
                let w = play(job);
                let line = serde_json::to_string(&w).unwrap();
                let mut f = out.lock().unwrap();
                writeln!(f, "{line}").unwrap();
                f.flush().unwrap();
                let d = done.fetch_add(1, Ordering::SeqCst) + 1;
                eprintln!(
                    "[{d}/{total}] {}p {} {} seed {}: winner {:?} scores {:?} ({:.1}s)",
                    w.players,
                    w.size,
                    if w.teams > 0 { "teams" } else { "ffa" },
                    w.seed,
                    w.winner,
                    w.pilots.iter().map(|p| p.score).collect::<Vec<_>>(),
                    w.wall_s
                );
            });
        }
    });
}

fn class_name(c: ShipClass) -> String {
    c.stats().name.to_string()
}

fn rates(g: &Game, p: PlayerId) -> (f64, f64) {
    let mut m = 0.0;
    let mut e = 0.0;
    for (i, b) in g.bodies.iter().enumerate() {
        if b.owner == Some(p) {
            let (pm, pe, _) = g.production(BodyId(i as u32));
            m += pm;
            e += pe;
        }
    }
    (m, e)
}

/// Leading side (player in FFA, team in team wars) by score.
fn leader(g: &Game) -> u32 {
    if g.team_game() {
        (0..g.settings.teams as u8).max_by_key(|&t| (g.team_score(t), std::cmp::Reverse(t))).unwrap_or(0) as u32
    } else {
        g.players.iter().max_by_key(|p| (g.score(p.id), std::cmp::Reverse(p.id))).map(|p| p.id).unwrap_or(0)
    }
}

fn play(job: &Job) -> WarOut {
    let wall = std::time::Instant::now();
    let settings = Settings { name: "warsim".into(), size: parse_size(&job.size), max_players: job.players, length: job.days * DAY, speed: 1.0, teams: job.teams };
    let mut g = Game::new("SIM".into(), settings, job.seed, "Bot 0");
    g.players[0].bot = Some(job.bots[0].clone());
    for i in 1..job.players {
        g.add_player(&format!("Bot {i}"), Some(job.bots[i].clone())).unwrap();
    }
    g.start();
    let n = job.players;

    // planets ordered by distance from the star, for "home position"
    let mut planets: Vec<(f64, BodyId)> = g
        .system
        .bodies
        .iter()
        .filter(|b| matches!(b.kind, BodyKind::Planet | BodyKind::GasGiant))
        .map(|b| (b.orbit.map(|o| o.a).unwrap_or(0.0), b.id))
        .collect();
    planets.sort_by(|a, b| a.0.total_cmp(&b.0));

    let mut pilots: Vec<PlayerOut> = g
        .players
        .iter()
        .map(|p| {
            let home = p.home.unwrap();
            let rank = planets.iter().position(|x| x.1 == home).unwrap_or(0);
            PlayerOut {
                id: p.id,
                bot: p.bot.clone().unwrap_or_default(),
                team: p.team,
                home_kind: if g.system.body(home).kind == BodyKind::GasGiant { "giant" } else { "rocky" }.into(),
                home_rank: rank,
                home_pos: rank as f64 / (planets.len().max(2) - 1) as f64,
                ..Default::default()
            }
        })
        .collect();

    let mut owners: Vec<Option<PlayerId>> = g.bodies.iter().map(|b| b.owner).collect();
    let mut known: HashMap<ShipId, (PlayerId, ShipClass)> = g.ships.iter().map(|s| (s.id, (s.owner, s.class))).collect();
    let mut battles_seen: std::collections::HashSet<u32> = Default::default();
    let mut leaders: Vec<u32> = vec![];
    let checkpoints = [1.0 * DAY, 3.0 * DAY, 7.0 * DAY];
    let mut cp = 0;
    let mut next_tl = 0.0;
    let end = g.end_t;
    let mut t = 0.0;
    loop {
        if t >= next_tl {
            for p in &mut pilots {
                p.timeline.push(g.score(p.id));
            }
            leaders.push(leader(&g));
            next_tl += TIMELINE;
        }
        if t >= end || g.status == Status::Ended {
            break;
        }
        t = (t + SAMPLE).min(end);
        g.advance_to(t);
        for b in &g.battles {
            battles_seen.insert(b.id);
        }
        let mut now: HashMap<ShipId, (PlayerId, ShipClass)> = HashMap::with_capacity(g.ships.len());
        for s in &g.ships {
            now.insert(s.id, (s.owner, s.class));
            if !known.contains_key(&s.id) {
                *pilots[s.owner as usize].built.entry(class_name(s.class)).or_default() += 1;
            }
        }
        for (id, (owner, class)) in &known {
            if !now.contains_key(id) {
                *pilots[*owner as usize].lost.entry(class_name(*class)).or_default() += 1;
            }
        }
        known = now;
        for (i, b) in g.bodies.iter().enumerate() {
            if b.owner != owners[i] {
                if let Some(o) = owners[i] {
                    pilots[o as usize].lost_bodies += 1;
                }
                if let Some(n) = b.owner {
                    if owners[i].is_some() {
                        pilots[n as usize].took_enemy += 1;
                    } else {
                        pilots[n as usize].took_neutral += 1;
                    }
                }
                owners[i] = b.owner;
            }
        }
        while cp < checkpoints.len() && t >= checkpoints[cp] {
            for p in &mut pilots {
                let (m, e) = rates(&g, p.id);
                p.matter_rate.push(m);
                p.energy_rate.push(e);
                p.matter_bank.push(g.players[p.id as usize].matter);
                p.energy_bank.push(g.players[p.id as usize].energy);
            }
            cp += 1;
        }
        for p in &mut pilots {
            let held = g.bodies.iter().filter(|b| b.owner == Some(p.id)).count();
            if held > 1 && p.first_capture_day.is_none() {
                p.first_capture_day = Some(t / DAY);
            }
            if held == 0 && p.out_day.is_none() {
                p.out_day = Some(t / DAY);
            }
            let ships = known.values().filter(|v| v.0 == p.id).count();
            p.peak_ships = p.peak_ships.max(ships);
        }
    }
    // the war's own end-of-game result
    for p in &mut pilots {
        let id = p.id;
        let (m, e) = rates(&g, id);
        while p.matter_rate.len() < 3 {
            p.matter_rate.push(f64::NAN);
            p.energy_rate.push(f64::NAN);
            p.matter_bank.push(f64::NAN);
            p.energy_bank.push(f64::NAN);
        }
        p.matter_rate.push(m);
        p.energy_rate.push(e);
        p.matter_bank.push(g.players[id as usize].matter);
        p.energy_bank.push(g.players[id as usize].energy);
        p.score = g.score(id);
        p.bodies = g.bodies.iter().filter(|b| b.owner == Some(id)).count();
        p.planets = g.bodies.iter().enumerate().filter(|(i, b)| b.owner == Some(id) && matches!(g.system.bodies[*i].kind, BodyKind::Planet | BodyKind::GasGiant)).count();
        p.kept_home = g.players[id as usize].home.is_some_and(|h| g.bodies[h.0 as usize].owner == Some(id));
        p.final_ships = g.ships.iter().filter(|s| s.owner == id).count();
        p.techs = g.players[id as usize].techs.len();
        p.stations = g.stations.iter().filter(|s| s.owner == id).count();
        p.won = if g.team_game() { g.winning_team == Some(p.team) } else { g.winner == Some(id) };
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| (std::cmp::Reverse(pilots[i].score), i));
    for (r, &i) in order.iter().enumerate() {
        pilots[i].rank = r + 1;
    }
    let mut side_scores: Vec<i64> =
        if g.team_game() { (0..g.settings.teams as u8).map(|t| g.team_score(t) as i64).collect() } else { pilots.iter().map(|p| p.score as i64).collect() };
    side_scores.sort_by(|a, b| b.cmp(a));
    let final_leader = *leaders.last().unwrap();
    let lead_changes = leaders.windows(2).filter(|w| w[0] != w[1]).count() as u32;
    let lock = leaders.iter().rposition(|&l| l != final_leader).map(|i| i + 1).unwrap_or(0);
    WarOut {
        rules: g.rules,
        seed: job.seed,
        size: job.size.clone(),
        players: n,
        teams: job.teams,
        days: job.days,
        bodies: g.system.bodies.iter().filter(|b| !matches!(b.kind, BodyKind::Star | BodyKind::Outpost)).count(),
        planets: planets.len(),
        winner: g.winner,
        winning_team: g.winning_team,
        margin: side_scores[0] - side_scores.get(1).copied().unwrap_or(0),
        lead_changes,
        locked_day: lock as f64 * TIMELINE / DAY,
        neutral_left: g.bodies.iter().enumerate().filter(|(i, b)| b.owner.is_none() && !b.shattered && !b.gone && !matches!(g.system.bodies[*i].kind, BodyKind::Star | BodyKind::Outpost)).count(),
        battles: battles_seen.len() as u32,
        wall_s: wall.elapsed().as_secs_f64(),
        pilots,
    }
}

// ---------------------------------------------------------------------------------------------
// duel

fn duel(o: &Opts) {
    let budget = o.num("budget", 3000.0);
    let reps = o.num("reps", 5.0) as u64;
    let classes: Vec<ShipClass> = o.list("classes", "interceptor,gunship,bomber,dreadnought").iter().map(|s| parse_class(s)).collect();
    let probe = Game::new("P".into(), Settings { name: "p".into(), size: SystemSize::Medium, max_players: 2, length: DAY, speed: 1.0, teams: 0 }, 1, "a");
    // --scale dreadnought=0.5 sizes fleets as if that class cost half as much (for "what price is fair?")
    let scales: Vec<(ShipClass, f64)> = o
        .list("scale", "")
        .iter()
        .filter_map(|kv| kv.split_once('=').map(|(k, v)| (parse_class(k), v.parse().unwrap_or_else(|_| usage()))))
        .collect();
    let cost = |c: ShipClass| {
        let (m, e, _) = probe.ship_cost(c);
        (m + e) * scales.iter().find(|s| s.0 == c).map(|s| s.1).unwrap_or(1.0)
    };
    println!("Equal-cost duels, budget {budget:.0} matter+energy per side, {reps} seeds each, no research.");
    println!("Cell = row class's share of wins, then avg % of its hull left (draws count half).\n");
    for &c in &classes {
        let n = (budget / cost(c)).round().max(1.0) as usize;
        println!("  {:<12} {:>3} ships ({:.0} each)", c.stats().name, n, cost(c));
    }
    print!("\n{:<12}", "");
    for &c in &classes {
        print!(" {:>16}", c.stats().name);
    }
    println!();
    for &a in &classes {
        print!("{:<12}", a.stats().name);
        for &b in &classes {
            let mut wins = 0.0;
            let mut surv = 0.0;
            for r in 0..reps {
                // alternate sides so a first-mover bias in the battle code cancels out
                let (wa, sa) = fight(a, b, budget, 100 + r, r % 2 == 1, &cost);
                wins += wa;
                surv += sa;
            }
            print!(" {:>8.0}% {:>5.0}%", 100.0 * wins / reps as f64, 100.0 * surv / reps as f64);
        }
        println!();
    }
}

/// One equal-cost fight at a neutral moon. Returns (win share of `a`, share of a's cost left).
fn fight(a: ShipClass, b: ShipClass, budget: f64, seed: u64, swap: bool, cost: &dyn Fn(ShipClass) -> f64) -> (f64, f64) {
    let (pa, pb): (PlayerId, PlayerId) = if swap { (1, 0) } else { (0, 1) };
    let mut g = Game::new("D".into(), Settings { name: "d".into(), size: SystemSize::Medium, max_players: 2, length: 7.0 * DAY, speed: 1.0, teams: 0 }, seed, "A");
    g.add_player("B", None).unwrap();
    g.start();
    g.ships.clear();
    let at = g.system.bodies.iter().find(|x| x.kind == BodyKind::Moon && g.bodies[x.id.0 as usize].owner.is_none()).map(|x| x.id).expect("neutral moon");
    let na = (budget / cost(a)).round().max(1.0) as usize;
    let nb = (budget / cost(b)).round().max(1.0) as usize;
    for _ in 0..na {
        g.spawn(pa, a, at);
    }
    for _ in 0..nb {
        g.spawn(pb, b, at);
    }
    let t0 = g.t;
    let mut t = t0;
    let alive = |g: &Game, p: PlayerId| g.ships.iter().filter(|s| s.owner == p && s.parked_at() == Some(at)).count();
    while t < t0 + 2.0 * DAY && alive(&g, pa) > 0 && alive(&g, pb) > 0 {
        t += 300.0;
        g.advance_to(t);
    }
    let (la, lb) = (alive(&g, pa), alive(&g, pb));
    let hp_left: f64 = g.ships.iter().filter(|s| s.owner == pa && s.parked_at() == Some(at)).map(|s| s.hp / s.stats().hp).sum::<f64>() / na as f64;
    let w = match (la > 0, lb > 0) {
        (true, false) => 1.0,
        (false, true) => 0.0,
        _ => 0.5,
    };
    (w, hp_left)
}

// ---------------------------------------------------------------------------------------------
// report

fn mean(v: &[f64]) -> f64 {
    let v: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        f64::NAN
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

fn report(files: &[String]) {
    let mut wars: Vec<WarOut> = vec![];
    for f in files {
        let r = std::io::BufReader::new(std::fs::File::open(f).expect("open"));
        for line in r.lines().map_while(Result::ok) {
            if let Ok(w) = serde_json::from_str::<WarOut>(&line) {
                wars.push(w);
            }
        }
    }
    if wars.is_empty() {
        eprintln!("no wars");
        return;
    }
    println!("{} wars (rules {:?})\n", wars.len(), wars.iter().map(|w| w.rules).collect::<std::collections::BTreeSet<_>>());

    // per format
    let mut groups: BTreeMap<(usize, String, usize), Vec<&WarOut>> = BTreeMap::new();
    for w in &wars {
        groups.entry((w.players, w.size.clone(), w.teams)).or_default().push(w);
    }
    println!("== Wars by format ==");
    println!("{:<22} {:>4} {:>7} {:>7} {:>8} {:>7} {:>8} {:>9} {:>8}", "format", "n", "margin", "leadch", "lock d", "neutral", "battles", "1st cap d", "wall s");
    for ((p, s, t), ws) in &groups {
        let f = |g: &dyn Fn(&WarOut) -> f64| mean(&ws.iter().map(|w| g(w)).collect::<Vec<_>>());
        let cap = mean(&ws.iter().flat_map(|w| w.pilots.iter().map(|p| p.first_capture_day.unwrap_or(f64::NAN))).collect::<Vec<_>>());
        println!(
            "{:<22} {:>4} {:>7.1} {:>7.1} {:>8.1} {:>7.1} {:>8.1} {:>9.2} {:>8.1}",
            format!("{p}p {s} {}", if *t > 0 { format!("{t} teams") } else { "ffa".into() }),
            ws.len(),
            f(&|w| w.margin as f64),
            f(&|w| w.lead_changes as f64),
            f(&|w| w.locked_day),
            f(&|w| w.neutral_left as f64),
            f(&|w| w.battles as f64),
            cap,
            f(&|w| w.wall_s)
        );
    }

    let all: Vec<(&WarOut, &PlayerOut)> = wars.iter().flat_map(|w| w.pilots.iter().map(move |p| (w, p))).collect();
    let ffa: Vec<_> = all.iter().filter(|(w, _)| w.teams == 0).copied().collect();
    let table = |title: &str, rows: Vec<(String, Vec<(&WarOut, &PlayerOut)>)>| {
        println!("\n== {title} (FFA) ==");
        println!("{:<14} {:>6} {:>8} {:>8} {:>8} {:>9} {:>8} {:>8}", "", "pilots", "win %", "fair %", "avg rank", "score", "kept h%", "out %");
        for (k, ps) in rows {
            if ps.is_empty() {
                continue;
            }
            let n = ps.len() as f64;
            let win = ps.iter().filter(|(_, p)| p.won).count() as f64 / n;
            let fair = mean(&ps.iter().map(|(w, _)| 1.0 / w.players as f64).collect::<Vec<_>>());
            println!(
                "{:<14} {:>6} {:>7.1}% {:>7.1}% {:>8.2} {:>9.1} {:>7.0}% {:>7.0}%",
                k,
                ps.len(),
                100.0 * win,
                100.0 * fair,
                mean(&ps.iter().map(|(_, p)| p.rank as f64).collect::<Vec<_>>()),
                mean(&ps.iter().map(|(_, p)| p.score as f64).collect::<Vec<_>>()),
                100.0 * ps.iter().filter(|(_, p)| p.kept_home).count() as f64 / n,
                100.0 * ps.iter().filter(|(_, p)| p.out_day.is_some()).count() as f64 / n,
            );
        }
    };
    table("Home kind", ["rocky", "giant"].iter().map(|k| (k.to_string(), ffa.iter().filter(|(_, p)| p.home_kind == *k).copied().collect())).collect());
    table(
        "Home orbit (0 = innermost planet)",
        [(0.0f64, 0.25f64), (0.25, 0.5), (0.5, 0.75), (0.75, 1.01)]
            .iter()
            .map(|&(a, b)| (format!("{a:.2}-{:.2}", b.min(1.0)), ffa.iter().filter(|(_, p)| p.home_pos >= a && p.home_pos < b).copied().collect()))
            .collect(),
    );
    table("Bot difficulty", ["easy", "normal", "hard"].iter().map(|k| (k.to_string(), ffa.iter().filter(|(_, p)| p.bot == *k).copied().collect())).collect());
    table("Slot", (0..8u32).map(|i| (format!("slot {i}"), ffa.iter().filter(|(_, p)| p.id == i).copied().collect())).collect());

    // snowball: leader at day X goes on to win?
    println!("\n== Snowballing (FFA): chance the side leading at day D wins ==");
    for d in [1.0, 2.0, 3.0, 5.0, 7.0, 10.0] {
        let idx = (d * DAY / TIMELINE) as usize;
        let mut hit = 0;
        let mut n = 0;
        for w in wars.iter().filter(|w| w.teams == 0) {
            if w.pilots[0].timeline.len() <= idx {
                continue;
            }
            let best = w.pilots.iter().max_by_key(|p| (p.timeline[idx], std::cmp::Reverse(p.id))).unwrap();
            if best.timeline[idx] == 0 {
                continue;
            }
            n += 1;
            if best.won {
                hit += 1;
            }
        }
        if n > 0 {
            println!("  day {d:>4}: {:>5.1}% of {n}", 100.0 * hit as f64 / n as f64);
        }
    }

    // ships
    println!("\n== Ships (all wars): built and lost per pilot ==");
    let mut built: BTreeMap<String, f64> = BTreeMap::new();
    let mut lost: BTreeMap<String, f64> = BTreeMap::new();
    let mut built_w: BTreeMap<String, f64> = BTreeMap::new();
    let mut nw = 0.0f64;
    for (_, p) in &all {
        for (k, v) in &p.built {
            *built.entry(k.clone()).or_default() += *v as f64;
            if p.won {
                *built_w.entry(k.clone()).or_default() += *v as f64;
            }
        }
        for (k, v) in &p.lost {
            *lost.entry(k.clone()).or_default() += *v as f64;
        }
        if p.won {
            nw += 1.0;
        }
    }
    let np = all.len() as f64;
    println!("{:<12} {:>8} {:>8} {:>9} {:>11}", "class", "built", "lost", "lost/blt", "winners blt");
    let mut keys: Vec<&String> = built.keys().chain(lost.keys()).collect();
    keys.sort();
    keys.dedup();
    for k in keys {
        let b = built.get(k).copied().unwrap_or(0.0);
        let l = lost.get(k).copied().unwrap_or(0.0);
        println!("{:<12} {:>8.2} {:>8.2} {:>9} {:>11.2}", k, b / np, l / np, if b > 0.0 { format!("{:.2}", l / b) } else { "-".into() }, built_w.get(k).copied().unwrap_or(0.0) / nw.max(1.0));
    }

    println!("\n== Economy (all pilots): matter/h and energy/h ==");
    for (i, d) in ["day 1", "day 3", "day 7", "end"].iter().enumerate() {
        let m = mean(&all.iter().map(|(_, p)| p.matter_rate.get(i).copied().unwrap_or(f64::NAN)).collect::<Vec<_>>());
        let e = mean(&all.iter().map(|(_, p)| p.energy_rate.get(i).copied().unwrap_or(f64::NAN)).collect::<Vec<_>>());
        let mw = mean(&all.iter().filter(|(_, p)| p.won).map(|(_, p)| p.matter_rate.get(i).copied().unwrap_or(f64::NAN)).collect::<Vec<_>>());
        let mb = mean(&all.iter().map(|(_, p)| p.matter_bank.get(i).copied().unwrap_or(f64::NAN)).collect::<Vec<_>>());
        let eb = mean(&all.iter().map(|(_, p)| p.energy_bank.get(i).copied().unwrap_or(f64::NAN)).collect::<Vec<_>>());
        println!("  {d:<6} matter {m:>7.1}/h  energy {e:>7.1}/h   banked {mb:>8.0} m {eb:>8.0} e   (winners' matter {mw:>7.1}/h)");
    }
    println!(
        "  peak ships {:.1}, final ships {:.1}, techs {:.1}, stations {:.1}",
        mean(&all.iter().map(|(_, p)| p.peak_ships as f64).collect::<Vec<_>>()),
        mean(&all.iter().map(|(_, p)| p.final_ships as f64).collect::<Vec<_>>()),
        mean(&all.iter().map(|(_, p)| p.techs as f64).collect::<Vec<_>>()),
        mean(&all.iter().map(|(_, p)| p.stations as f64).collect::<Vec<_>>())
    );
    println!(
        "  bodies taken per pilot: {:.1} neutral, {:.1} from enemies; bodies lost {:.1}",
        mean(&all.iter().map(|(_, p)| p.took_neutral as f64).collect::<Vec<_>>()),
        mean(&all.iter().map(|(_, p)| p.took_enemy as f64).collect::<Vec<_>>()),
        mean(&all.iter().map(|(_, p)| p.lost_bodies as f64).collect::<Vec<_>>())
    );
    println!(
        "  planets per pilot at the end {:.2} (system has {:.2} per pilot)",
        mean(&all.iter().map(|(_, p)| p.planets as f64).collect::<Vec<_>>()),
        mean(&wars.iter().map(|w| w.planets as f64 / w.players as f64).collect::<Vec<_>>())
    );
}

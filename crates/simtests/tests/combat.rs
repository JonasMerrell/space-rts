//! Property tests for velocity-dependent combat (`sim::combat`) and asteroid impact energy.
use sim::combat::*;
use sim::game::{ShipClass, Weapon};
use sim::system::Rng;
use sim::{v2, V2};
use simtests::*;

fn ship(rng: &mut Rng, key: u64, owner: u32) -> Combatant {
    let classes = [ShipClass::Interceptor, ShipClass::Gunship, ShipClass::Bomber, ShipClass::Dreadnought, ShipClass::Scout, ShipClass::Tanker];
    let class = classes[rng.below(classes.len() as u32) as usize];
    let st = class.stats();
    Combatant {
        key, owner, side: owner, class: Some(class), weapon: st.weapon, attack: st.attack, hp: st.hp,
        pos: rand_dir(rng) * rng.range(0.0, 6000.0), vel: rand_dir(rng) * rng.range(0.0, 20.0),
        size: size(class), evade: evasion(class), dmg_mult: 1.0, acc_mult: 1.0,
    }
}

#[test]
fn lead_time_is_the_earliest_intercept() {
    forall("lead", |i, rng| {
        let r = rand_dir(rng) * rng.range(10.0, 5000.0);
        let u = rand_dir(rng) * rng.range(0.0, 40.0);
        let s = KINETIC_SPEED;
        match lead_time(r, u, s) {
            Some(t) => {
                assert!(t > 0.0, "case {i}");
                let miss = ((r + u * t).len() - s * t).abs();
                assert!(miss < 1e-6 * (s * t).max(1.0), "case {i}: misses by {miss}");
                // nothing earlier works: sample the interval before t
                for k in 1..50 {
                    let tt = t * k as f64 / 50.0;
                    assert!((r + u * tt).len() > s * tt - 1e-6 * s * t, "case {i}: earlier intercept at {tt}");
                }
            }
            None => {
                // no catching it: the target outruns the slug in every direction it can be met
                assert!(u.len() >= s * 0.999 || r.dot(u) > 0.0, "case {i}: catchable target rejected (r={r:?} u={u:?})");
            }
        }
    });
}

#[test]
fn resolve_is_order_independent_and_sane() {
    forall("resolve", |i, rng| {
        let n = 2 + rng.below(10) as usize;
        let cs: Vec<Combatant> = (0..n).map(|k| { let o = rng.below(3); ship(rng, k as u64 + 1, o) }).collect();
        let d = resolve(&cs, 60.0);
        assert_eq!(d.len(), n);
        for (k, x) in d.iter().enumerate() {
            assert!(x.is_finite() && *x >= 0.0, "case {i}: damage {x}");
            // nobody can be hurt if no enemy is armed and in range of them
            let threatened = cs.iter().any(|a| a.owner != cs[k].owner && a.attack > 0.0 && (a.pos - cs[k].pos).len() <= range(a.weapon));
            if !threatened {
                assert_eq!(*x, 0.0, "case {i}: {k} took damage with no enemy in range");
            }
        }
        // a random permutation gives the same per-combatant damage
        let mut idx: Vec<usize> = (0..n).collect();
        for k in (1..n).rev() {
            let j = rng.below(k as u32 + 1) as usize;
            idx.swap(k, j);
        }
        let shuffled: Vec<Combatant> = idx.iter().map(|&k| cs[k]).collect();
        let ds = resolve(&shuffled, 60.0);
        for (pos, &k) in idx.iter().enumerate() {
            assert_eq!(ds[pos].to_bits(), d[k].to_bits(), "case {i}: order changed combatant {k}'s damage");
        }
    });
}

#[test]
fn single_side_never_takes_damage() {
    forall("one-side", |i, rng| {
        let cs: Vec<Combatant> = (0..6).map(|k| ship(rng, k + 1, 7)).collect();
        assert!(resolve(&cs, 3600.0).iter().all(|&x| x == 0.0), "case {i}: friendly fire");
    });
}

#[test]
fn beams_weaken_with_range_but_never_miss() {
    let c = ShipClass::Gunship;
    let mut last = f64::INFINITY;
    for k in 0..=40 {
        let d = ENERGY_RANGE * k as f64 / 40.0;
        let s = shot(Weapon::Energy, v2(d.max(1.0), 0.0), v2(0.0, 30.0), size(c), evasion(c)).unwrap();
        assert_eq!(s.hit, 1.0);
        assert!(s.factor <= last + 1e-12, "beam stronger at {d} km");
        last = s.factor;
    }
}

#[test]
fn slugs_hurt_more_head_on_than_in_a_stern_chase() {
    forall("head-on", |i, rng| {
        let c = ShipClass::Gunship;
        let d = rng.range(100.0, KINETIC_RANGE);
        let v = rng.range(1.0, 20.0);
        let r = v2(d, 0.0);
        let head = shot(Weapon::Kinetic, r, v2(-v, 0.0), size(c), evasion(c)).unwrap();
        if let Some(tail) = shot(Weapon::Kinetic, r, v2(v, 0.0), size(c), evasion(c)) {
            assert!(head.factor > tail.factor, "case {i}");
            assert!(head.tof < tail.tof, "case {i}");
        }
    });
}

#[test]
fn small_ships_dodge_slugs_better_than_big_ones() {
    // at the same geometry a scout (small, nimble) is hit less than a dreadnought
    let r = v2(2000.0, 0.0);
    let u = v2(0.0, 5.0);
    let hit = |c: ShipClass| shot(Weapon::Kinetic, r, u, size(c), evasion(c)).unwrap().hit;
    assert!(hit(ShipClass::Scout) < hit(ShipClass::Dreadnought));
}

#[test]
fn impact_energy_is_half_m_v_squared() {
    forall("impact", |i, rng| {
        let (m, v) = (rng.range(1.0, 1e12), rng.range(0.0, 80.0));
        let e = sim::asteroid::impact_energy(m, v);
        assert!(close(e, 0.5 * m * v * v, 1e-12, 0.0), "case {i}");
    });
    assert_eq!(sim::asteroid::impact_energy(5.0, 0.0), 0.0);
    let _ = V2::ZERO;
}

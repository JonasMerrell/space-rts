//! Two-body orbital mechanics: universal-variable propagation, conic elements, Lambert's problem.
//! Units: km, s, km/s, km³/s².
use crate::v2::{v2, V2};
use std::f64::consts::{PI, TAU};

/// Stumpff functions C(z), S(z) with series near zero.
pub fn stumpff(z: f64) -> (f64, f64) {
    if z.abs() < 1e-6 {
        (0.5 - z / 24.0 + z * z / 720.0, 1.0 / 6.0 - z / 120.0 + z * z / 5040.0)
    } else if z > 0.0 {
        let s = z.sqrt();
        ((1.0 - s.cos()) / z, (s - s.sin()) / (s * s * s))
    } else {
        let s = (-z).sqrt();
        ((s.cosh() - 1.0) / -z, (s.sinh() - s) / (s * s * s))
    }
}

/// Propagate a two-body state by `dt` seconds (any conic, either direction).
pub fn propagate(mu: f64, r0: V2, v0: V2, dt: f64) -> (V2, V2) {
    if dt == 0.0 {
        return (r0, v0);
    }
    let r0m = r0.len();
    let v0m2 = v0.len2();
    let alpha = 2.0 / r0m - v0m2 / mu; // 1/a
    let mut dt = dt;
    if alpha > 1e-12 {
        // ellipse: reduce by whole periods for robustness
        let period = TAU / (mu * alpha * alpha * alpha).sqrt();
        dt %= period;
    }
    let smu = mu.sqrt();
    let vr0 = r0.dot(v0) / r0m;
    // F(chi) is strictly increasing (dF/dchi = r > 0): bracketed Newton with bisection fallback.
    let ff = |chi: f64| {
        let z = alpha * chi * chi;
        let (c, s) = stumpff(z);
        let f = r0m * vr0 / smu * chi * chi * c + (1.0 - alpha * r0m) * chi * chi * chi * s + r0m * chi - smu * dt;
        let df = r0m * vr0 / smu * chi * (1.0 - z * s) + (1.0 - alpha * r0m) * chi * chi * c + r0m;
        (f, df)
    };
    let (mut lo, mut hi) = (0.0, 0.0);
    let mut step = smu * dt.abs() / r0m.max(1e-9);
    if alpha > 1e-12 {
        step = step.min(TAU / alpha.sqrt());
    }
    step = step.max(1e-9);
    // expand the bracket in the direction of dt
    let dir = dt.signum();
    for _ in 0..200 {
        let x = dir * step;
        let (f, _) = ff(x);
        if !f.is_finite() || f * dir > 0.0 {
            if dir > 0.0 {
                hi = x
            } else {
                lo = x
            }
            break;
        }
        if dir > 0.0 {
            lo = x
        } else {
            hi = x
        }
        step *= 2.0;
    }
    if dir > 0.0 && hi <= lo {
        hi = lo + step;
    }
    if dir < 0.0 && lo >= hi {
        lo = hi - step;
    }
    let mut chi = 0.5 * (lo + hi);
    let mut dx_old = hi - lo;
    for _ in 0..300 {
        let (f, df) = ff(chi);
        if !f.is_finite() {
            hi = chi;
            chi = 0.5 * (lo + hi);
            continue;
        }
        if f > 0.0 {
            hi = chi
        } else {
            lo = chi
        }
        // rtsafe: fall back to bisection when Newton leaves the bracket or converges slowly
        let mut next = chi - f / df;
        if !(next > lo && next < hi) || !next.is_finite() || (f / df).abs() * 2.0 > dx_old {
            next = 0.5 * (lo + hi);
        }
        dx_old = (next - chi).abs();
        let done = (next - chi).abs() <= 1e-12 * chi.abs().max(1.0) || hi - lo <= 1e-12 * chi.abs().max(1.0);
        chi = next;
        if done {
            break;
        }
    }
    let z = alpha * chi * chi;
    let (c, s) = stumpff(z);
    let f = 1.0 - chi * chi / r0m * c;
    let g = dt - chi * chi * chi / smu * s;
    let r = r0 * f + v0 * g;
    let rm = r.len();
    let fdot = smu / (rm * r0m) * (alpha * chi * chi * chi * s - chi);
    let gdot = 1.0 - chi * chi / rm * c;
    (r, r0 * fdot + v0 * gdot)
}

/// Shape of a conic computed from a state vector.
#[derive(Clone, Copy, Debug)]
pub struct Conic {
    pub mu: f64,
    /// semi-major axis (negative for hyperbola)
    pub a: f64,
    pub e: f64,
    /// eccentricity vector (points at periapsis)
    pub evec: V2,
    /// specific angular momentum (z); sign = direction (positive = CCW)
    pub h: f64,
    pub rp: f64,
}

impl Conic {
    pub fn from_state(mu: f64, r: V2, v: V2) -> Conic {
        let rm = r.len();
        let h = r.cross(v);
        // e = ((v² - mu/r) r - (r·v) v) / mu
        let evec = (r * (v.len2() - mu / rm) - v * r.dot(v)) / mu;
        let e = evec.len();
        let energy = v.len2() / 2.0 - mu / rm;
        let a = if energy.abs() > 1e-12 { -mu / (2.0 * energy) } else { f64::INFINITY };
        let p = h * h / mu;
        let rp = p / (1.0 + e);
        Conic { mu, a, e, evec, h, rp }
    }
    pub fn apoapsis(&self) -> f64 {
        if self.e < 1.0 {
            self.a * (1.0 + self.e)
        } else {
            f64::INFINITY
        }
    }
    pub fn period(&self) -> Option<f64> {
        (self.e < 1.0).then(|| TAU * (self.a.powi(3) / self.mu).sqrt())
    }
    /// Signed true anomaly of position r.
    pub fn true_anomaly(&self, r: V2) -> f64 {
        let ehat = if self.e > 1e-9 { self.evec / self.e } else { v2(1.0, 0.0) };
        let s = self.h.signum();
        (ehat.cross(r) * s).atan2(ehat.dot(r))
    }
    /// Time from true anomaly 0 (periapsis) to `nu` (signed, ellipse in [-T/2, T/2]).
    pub fn time_since_periapsis(&self, nu: f64) -> f64 {
        let e = self.e;
        if e < 1.0 {
            let ea = 2.0 * (((1.0 - e) / (1.0 + e)).sqrt() * (nu / 2.0).tan()).atan();
            let m = ea - e * ea.sin();
            m / (self.mu / self.a.powi(3)).sqrt()
        } else {
            let ha = 2.0 * (((e - 1.0) / (e + 1.0)).sqrt() * (nu / 2.0).tan()).atanh();
            let m = e * ha.sinh() - ha;
            m / (self.mu / (-self.a).powi(3)).sqrt()
        }
    }
    /// Seconds until the next periapsis passage from state r (None for receding hyperbola).
    pub fn time_to_periapsis(&self, r: V2, v: V2) -> Option<f64> {
        let nu = self.true_anomaly(r);
        let tsp = self.time_since_periapsis(nu);
        if self.e < 1.0 {
            let t = self.period().unwrap();
            let mut dt = -tsp;
            if dt < 0.0 {
                dt += t;
            }
            if r.dot(v).abs() < 1e-9 && self.e < 1e-6 {
                return Some(0.0);
            }
            Some(dt)
        } else if r.dot(v) < 0.0 {
            Some(-tsp)
        } else {
            None
        }
    }
}

/// Lambert's problem (single revolution, direction given by `ccw`). Returns (v1, v2).
pub fn lambert(mu: f64, r1: V2, r2: V2, tof: f64, ccw: bool) -> Option<(V2, V2)> {
    if tof <= 0.0 {
        return None;
    }
    let r1m = r1.len();
    let r2m = r2.len();
    let cosd = (r1.dot(r2) / (r1m * r2m)).clamp(-1.0, 1.0);
    let mut dth = cosd.acos();
    let cr = r1.cross(r2);
    if (ccw && cr < 0.0) || (!ccw && cr >= 0.0) {
        dth = TAU - dth;
    }
    if dth.abs() < 1e-6 || (dth - TAU).abs() < 1e-6 {
        return None;
    }
    let a = dth.sin() * (r1m * r2m / (1.0 - dth.cos())).sqrt();
    let y = |z: f64| {
        let (c, s) = stumpff(z);
        r1m + r2m + a * (z * s - 1.0) / c.sqrt()
    };
    let f = |z: f64| {
        let (c, s) = stumpff(z);
        let yy = y(z);
        if yy < 0.0 {
            return f64::NEG_INFINITY;
        }
        (yy / c).powf(1.5) * s + a * yy.sqrt() - mu.sqrt() * tof
    };
    let hi0 = 4.0 * PI * PI - 1e-6;
    let mut lo = -600.0;
    // y must be positive at lo; for a > 0 the lower bound is where y crosses zero
    if y(lo) < 0.0 {
        let (mut l, mut h) = (lo, hi0);
        if y(h) < 0.0 {
            return None;
        }
        for _ in 0..200 {
            let m = 0.5 * (l + h);
            if y(m) < 0.0 {
                l = m
            } else {
                h = m
            }
        }
        lo = h;
    }
    let mut hi = hi0;
    if f(lo) > 0.0 || f(hi) < 0.0 {
        return None;
    }
    for _ in 0..200 {
        let m = 0.5 * (lo + hi);
        if f(m) > 0.0 {
            hi = m
        } else {
            lo = m
        }
        if hi - lo < 1e-12 {
            break;
        }
    }
    let z = 0.5 * (lo + hi);
    let yy = y(z);
    let ff = 1.0 - yy / r1m;
    let g = a * (yy / mu).sqrt();
    let gdot = 1.0 - yy / r2m;
    let v1 = (r2 - r1 * ff) / g;
    let v2 = (r2 * gdot - r1) / g;
    (v1.x.is_finite() && v2.x.is_finite()).then_some((v1, v2))
}

/// Hyperbolic departure from a circular parking orbit of radius `rp` (CCW) such that the outgoing
/// asymptote velocity equals `vinf`. Returns (position, velocity) relative to the body at periapsis.
pub fn ejection(mu: f64, rp: f64, vinf: V2) -> (V2, V2) {
    let vi = vinf.len();
    let e = 1.0 + rp * vi * vi / mu;
    let th = (-1.0 / e).acos(); // true anomaly of asymptote
    // velocity direction at the asymptote in the perifocal frame (CCW motion)
    let dir = v2(-th.sin(), e + th.cos());
    let beta = vinf.angle() - dir.angle();
    let vp = (vi * vi + 2.0 * mu / rp).sqrt();
    let pos = V2::from_angle(beta) * rp;
    let vel = V2::from_angle(beta).perp() * vp;
    (pos, vel)
}

/// Δv to go from a circular orbit of radius rp to a hyperbola with excess speed vinf (or reverse).
pub fn oberth_dv(mu: f64, rp: f64, vinf: f64) -> f64 {
    (vinf * vinf + 2.0 * mu / rp).sqrt() - (mu / rp).sqrt()
}

pub fn circular_speed(mu: f64, r: f64) -> f64 {
    (mu / r).sqrt()
}

/// Solve Kepler's equation M = E - e sin E.
pub fn ecc_anomaly(m: f64, e: f64) -> f64 {
    let m = m.rem_euclid(TAU);
    let mut ea = if e < 0.8 { m } else { PI };
    for _ in 0..50 {
        let d = (ea - e * ea.sin() - m) / (1.0 - e * ea.cos());
        ea -= d;
        if d.abs() < 1e-13 {
            break;
        }
    }
    ea
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn propagate_circular_full_period() {
        let mu = 3.65e8;
        let r = 4e5;
        let v = (mu / r as f64).sqrt();
        let t = TAU * (r * r * r / mu as f64).sqrt();
        let (p, q) = propagate(mu, v2(r, 0.0), v2(0.0, v), t * 0.25);
        assert!((p - v2(0.0, r)).len() < 1.0, "{p:?}");
        assert!((q - v2(-v, 0.0)).len() < 1e-4);
    }

    #[test]
    fn propagate_hyperbola_roundtrip() {
        let mu = 2e5;
        let (r0, v0) = (v2(6000.0, 0.0), v2(0.0, 9.5));
        let (r1, v1) = propagate(mu, r0, v0, 5000.0);
        let (r2, v2_) = propagate(mu, r1, v1, -5000.0);
        assert!((r2 - r0).len() < 1e-3, "{r2:?}");
        assert!((v2_ - v0).len() < 1e-6);
    }

    #[test]
    fn lambert_matches_propagation() {
        let mu = 3.65e8;
        let r1 = v2(4e5, 0.0);
        let r2 = v2(-3e5, 5e5);
        let tof = 40_000.0;
        let (v1, v2_) = lambert(mu, r1, r2, tof, true).unwrap();
        let (rr, vv) = propagate(mu, r1, v1, tof);
        assert!((rr - r2).len() < 1.0, "{rr:?}");
        assert!((vv - v2_).len() < 1e-4);
    }

    #[test]
    fn ejection_asymptote() {
        let mu = 2e5;
        let vinf = v2(1.0, 2.5);
        let (p, v) = ejection(mu, 6000.0, vinf);
        let (_, vfar) = propagate(mu, p, v, 4e6);
        assert!((vfar.norm() - vinf.norm()).len() < 2e-3, "{vfar:?}");
        assert!((vfar.len() - vinf.len()).abs() < 0.01);
    }

    #[test]
    fn periapsis_time() {
        let mu = 2e5;
        let (r0, v0) = (v2(6000.0, 0.0), v2(0.0, 9.5));
        let (r1, v1) = propagate(mu, r0, v0, -3000.0);
        let c = Conic::from_state(mu, r1, v1);
        let t = c.time_to_periapsis(r1, v1).unwrap();
        assert!((t - 3000.0).abs() < 0.01, "{t}");
    }
}

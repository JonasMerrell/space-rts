//! Abuse limits for an internet-facing server: connection caps, login/register rate limits,
//! a cap on concurrent password hashes. Loopback peers (tests, the VM itself) are exempt from the
//! rate limits but not from the connection caps.
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct Limits {
    pub max_conns: usize,
    pub max_conns_per_ip: usize,
    /// (failed logins, window) per IP
    pub ip_fails: (usize, Duration),
    /// (failed logins, window) per account, from all IPs together
    pub account_fails: (usize, Duration),
    /// (new accounts, window) per IP
    pub registers: (usize, Duration),
    pub handshake_timeout: Duration,
    /// No message for this long → drop the connection (the app pings every 15 s).
    pub idle_timeout: Duration,
    pub write_timeout: Duration,
    /// Token bucket per connection: sustained messages per second and burst.
    pub msg_rate: f64,
    pub msg_burst: f64,
    /// Separate, slower bucket for messages that make the server serialize a whole game or
    /// grow saved state (Open, JoinGame, CreateGame, ListGames, Events, Chat). Over it → refused.
    pub heavy_rate: f64,
    pub heavy_burst: f64,
    /// Games one player may host that haven't ended.
    pub max_hosted: usize,
    pub max_hashing: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            max_conns: 512,
            max_conns_per_ip: 8,
            ip_fails: (10, Duration::from_secs(600)),
            account_fails: (20, Duration::from_secs(900)),
            registers: (3, Duration::from_secs(3600)),
            handshake_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(90),
            write_timeout: Duration::from_secs(30),
            msg_rate: 20.0,
            msg_burst: 100.0,
            heavy_rate: 0.5,
            heavy_burst: 15.0,
            max_hosted: 10,
            max_hashing: 2,
        }
    }
}

#[derive(Default)]
struct IpInfo {
    conns: usize,
    fails: VecDeque<Instant>,
    registers: VecDeque<Instant>,
}

#[derive(Default)]
struct State {
    total: usize,
    ips: HashMap<IpAddr, IpInfo>,
    accounts: HashMap<String, VecDeque<Instant>>,
    hashing: usize,
}

pub struct Guard {
    pub limits: Limits,
    st: Mutex<State>,
    hash_free: Condvar,
}

/// Held for the life of a connection; frees its slot when dropped.
pub struct Ticket {
    guard: Arc<Guard>,
    ip: IpAddr,
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let mut st = self.guard.state();
        st.total = st.total.saturating_sub(1);
        if let Some(i) = st.ips.get_mut(&self.ip) {
            i.conns = i.conns.saturating_sub(1);
        }
    }
}

/// Drop timestamps older than `window`; returns how many are left.
fn recent(q: &mut VecDeque<Instant>, window: Duration, now: Instant) -> usize {
    while q.front().is_some_and(|t| now.duration_since(*t) > window) {
        q.pop_front();
    }
    q.len()
}

fn wait_msg(q: &VecDeque<Instant>, window: Duration, now: Instant) -> String {
    let left = q.front().map(|t| window.saturating_sub(now.duration_since(*t))).unwrap_or(window);
    let m = left.as_secs().div_ceil(60).max(1);
    format!("try again in {m} min")
}

/// One stable stderr line per auth failure / limit hit, for fail2ban. Only the IP, never names.
pub fn log_fail(ip: IpAddr) {
    eprintln!("auth fail from {ip}");
}

pub fn log_limited(ip: IpAddr) {
    eprintln!("rate limited from {ip}");
}

impl Guard {
    pub fn new(limits: Limits) -> Arc<Guard> {
        Arc::new(Guard { limits, st: Mutex::new(State::default()), hash_free: Condvar::new() })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A slot for a new connection from `ip`, or None when it is over a cap.
    pub fn admit(self: &Arc<Guard>, ip: IpAddr) -> Option<Ticket> {
        let mut st = self.state();
        if st.total >= self.limits.max_conns {
            log_limited(ip);
            return None;
        }
        let i = st.ips.entry(ip).or_default();
        if i.conns >= self.limits.max_conns_per_ip {
            log_limited(ip);
            return None;
        }
        i.conns += 1;
        st.total += 1;
        // forget idle IPs now and then so the map can't grow without bound
        if st.ips.len() > 10_000 {
            let now = Instant::now();
            let l = self.limits.clone();
            st.ips.retain(|_, i| i.conns > 0 || recent(&mut i.fails, l.ip_fails.1, now) > 0 || recent(&mut i.registers, l.registers.1, now) > 0);
        }
        Some(Ticket { guard: self.clone(), ip })
    }

    /// May `ip` try a password for `user` right now?
    pub fn may_login(&self, ip: IpAddr, user: &str) -> Result<(), String> {
        let now = Instant::now();
        let mut st = self.state();
        let (n, w) = self.limits.account_fails;
        if let Some(q) = st.accounts.get_mut(user) {
            if recent(q, w, now) >= n {
                log_limited(ip);
                return Err(format!("too many wrong passwords for {user}, {}", wait_msg(q, w, now)));
            }
        }
        if ip.is_loopback() {
            return Ok(());
        }
        let (n, w) = self.limits.ip_fails;
        if let Some(i) = st.ips.get_mut(&ip) {
            if recent(&mut i.fails, w, now) >= n {
                log_limited(ip);
                return Err(format!("too many failed logins, {}", wait_msg(&i.fails, w, now)));
            }
        }
        Ok(())
    }

    pub fn login_failed(&self, ip: IpAddr, user: Option<&str>) {
        log_fail(ip);
        let now = Instant::now();
        let mut st = self.state();
        if let Some(u) = user {
            if st.accounts.len() > 10_000 {
                let w = self.limits.account_fails.1;
                st.accounts.retain(|_, q| recent(q, w, now) > 0);
            }
            st.accounts.entry(u.to_string()).or_default().push_back(now);
        }
        if !ip.is_loopback() {
            st.ips.entry(ip).or_default().fails.push_back(now);
        }
    }

    pub fn may_register(&self, ip: IpAddr) -> Result<(), String> {
        if ip.is_loopback() {
            return Ok(());
        }
        let now = Instant::now();
        let (n, w) = self.limits.registers;
        let mut st = self.state();
        let i = st.ips.entry(ip).or_default();
        if recent(&mut i.registers, w, now) >= n {
            log_limited(ip);
            return Err(format!("too many new accounts from your network, {}", wait_msg(&i.registers, w, now)));
        }
        Ok(())
    }

    pub fn registered(&self, ip: IpAddr) {
        if !ip.is_loopback() {
            self.state().ips.entry(ip).or_default().registers.push_back(Instant::now());
        }
    }

    /// Run a password hash, at most `max_hashing` at a time (argon2 is deliberately expensive).
    pub fn hashing<T>(&self, f: impl FnOnce() -> T) -> T {
        let mut st = self.state();
        while st.hashing >= self.limits.max_hashing.max(1) {
            st = self.hash_free.wait(st).unwrap_or_else(|e| e.into_inner());
        }
        st.hashing += 1;
        drop(st);
        let r = f();
        self.state().hashing -= 1;
        self.hash_free.notify_one();
        r
    }
}

/// Token bucket for one connection's message rate.
pub struct Bucket {
    tokens: f64,
    last: Instant,
}

impl Bucket {
    pub fn new(burst: f64) -> Bucket {
        Bucket { tokens: burst, last: Instant::now() }
    }
    pub fn take(&mut self, rate: f64, burst: f64) -> bool {
        self.take_n(1.0, rate, burst)
    }
    /// Take `n` tokens at once (e.g. one per 32 KiB sent); false (and nothing taken) if short.
    pub fn take_n(&mut self, n: f64, rate: f64, burst: f64) -> bool {
        self.refill(rate, burst);
        if self.tokens >= n {
            self.tokens -= n;
            true
        } else {
            false
        }
    }
    /// Refilled to the brim: forgetting this bucket changes nothing.
    pub fn is_full(&mut self, rate: f64, burst: f64) -> bool {
        self.refill(rate, burst);
        self.tokens >= burst
    }
    fn refill(&mut self, rate: f64, burst: f64) {
        let now = Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.last).as_secs_f64() * rate).min(burst);
        self.last = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_and_rates() {
        let g = Guard::new(Limits { max_conns_per_ip: 2, max_conns: 3, ..Limits::default() });
        let a: IpAddr = "203.0.113.5".parse().unwrap();
        let b: IpAddr = "198.51.100.7".parse().unwrap();
        let t1 = g.admit(a).unwrap();
        let _t2 = g.admit(a).unwrap();
        assert!(g.admit(a).is_none(), "per-IP cap");
        let _t3 = g.admit(b).unwrap();
        assert!(g.admit(b).is_none(), "global cap");
        drop(t1);
        assert!(g.admit(a).is_some(), "slot freed on drop");

        for _ in 0..10 {
            assert!(g.may_login(a, "ann").is_ok());
            g.login_failed(a, Some("ann"));
        }
        assert!(g.may_login(a, "bob").is_err(), "IP locked out after 10 failures");
        assert!(g.may_login(b, "bob").is_ok());
        for _ in 0..10 {
            g.login_failed(b, Some("ann"));
        }
        assert!(g.may_login("192.0.2.1".parse().unwrap(), "ann").is_err(), "account locked after 20 failures");

        for _ in 0..3 {
            assert!(g.may_register(b).is_ok());
            g.registered(b);
        }
        assert!(g.may_register(b).is_err());
        let lo: IpAddr = "127.0.0.1".parse().unwrap();
        for _ in 0..50 {
            g.registered(lo);
            g.login_failed(lo, None);
        }
        assert!(g.may_register(lo).is_ok() && g.may_login(lo, "zed").is_ok(), "loopback exempt");
    }

    #[test]
    fn bucket() {
        let l = Limits { msg_rate: 0.0, msg_burst: 5.0, ..Limits::default() };
        let mut b = Bucket::new(l.msg_burst);
        assert_eq!((0..8).filter(|_| b.take(l.msg_rate, l.msg_burst)).count(), 5);
    }
}

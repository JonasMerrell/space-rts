//! "Stay logged in" session tokens: minted on a password login that asks for one, used instead of
//! the password afterwards (also by the client backend's saved session), revoked by Logout,
//! capped per account, expiring when unused.
use proto::{decode, encode, ClientMsg, ServerMsg};
use server::Config;
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("tokens-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn start(data: &Path) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let cfg = Config::new(data.to_path_buf());
    std::thread::spawn(move || server::serve_with(l, cfg).unwrap());
    let end = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(&addr).is_err() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
    addr
}

/// One plaintext (loopback) connection: send Hello, return the Welcome token or the error.
fn hello(addr: &str, user: &str, pass: &str, token: Option<&str>, register: bool, stay: bool) -> (TcpStream, Result<Option<String>, String>) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let h = ClientMsg::Hello { user: user.into(), pass: pass.into(), register, invite: None, token: token.map(String::from), stay };
    s.write_all(encode(&h).as_bytes()).unwrap();
    let mut r = BufReader::new(s.try_clone().unwrap());
    let mut line = String::new();
    loop {
        line.clear();
        if r.read_line(&mut line).unwrap_or(0) == 0 {
            return (s, Err("closed".into()));
        }
        match decode::<ServerMsg>(&line) {
            Ok(ServerMsg::Welcome { token, .. }) => return (s, Ok(token)),
            Ok(ServerMsg::Error { msg }) => return (s, Err(msg)),
            _ => {}
        }
    }
}

fn sha256_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn token_login_logout_and_cap() {
    let data = temp_dir("flow");
    let addr = start(&data);
    // register + stay → a token; a plain login without `stay` gets none
    let (_c, t1) = hello(&addr, "ann", "pw-ann-1", None, true, true);
    let t1 = t1.unwrap().expect("token on register with stay");
    assert!(proto::ok_token(&t1), "{t1}");
    assert_eq!(hello(&addr, "ann", "pw-ann-1", None, false, false).1, Ok(None));
    // the token works without the password, again and again (reconnects), and gets no new token
    for _ in 0..3 {
        assert_eq!(hello(&addr, "ann", "", Some(&t1), false, false).1, Ok(None));
    }
    // only hashes are stored
    let accounts = std::fs::read_to_string(data.join("accounts.json")).unwrap();
    assert!(!accounts.contains(&t1) && accounts.contains(&sha256_hex(&t1)));
    // wrong token / other user's token / empty password without a token
    let other = "0".repeat(64);
    assert!(hello(&addr, "ann", "", Some(&other), false, false).1.unwrap_err().contains("expired"));
    let (_c, tb) = hello(&addr, "bob", "pw-bob-1", None, true, true);
    assert!(hello(&addr, "ann", "", Some(&tb.unwrap().unwrap()), false, false).1.is_err());
    assert!(hello(&addr, "ann", "", None, false, false).1.is_err());
    // malformed tokens are refused before any lookup
    assert!(hello(&addr, "ann", "", Some("not-hex"), false, false).1.is_err());

    // Logout revokes only the presented token
    let (_c, t2) = hello(&addr, "ann", "pw-ann-1", None, false, true);
    let t2 = t2.unwrap().unwrap();
    let (mut c, r) = hello(&addr, "ann", "", Some(&t2), false, false);
    assert!(r.is_ok());
    c.write_all(encode(&ClientMsg::Logout { token: t2.clone() }).as_bytes()).unwrap();
    // a round trip so the Logout has been handled
    c.write_all(encode(&ClientMsg::Ping).as_bytes()).unwrap();
    let mut r = BufReader::new(c);
    let mut line = String::new();
    while r.read_line(&mut line).unwrap() > 0 && !line.contains("Pong") {
        line.clear();
    }
    assert!(hello(&addr, "ann", "", Some(&t2), false, false).1.is_err(), "revoked");
    assert!(hello(&addr, "ann", "", Some(&t1), false, false).1.is_ok(), "the other device stays logged in");

    // 10 sessions per account: the least recently used goes
    std::thread::sleep(Duration::from_millis(20));
    let newer: Vec<String> = (0..10).map(|_| hello(&addr, "ann", "pw-ann-1", None, false, true).1.unwrap().unwrap()).collect();
    assert!(hello(&addr, "ann", "", Some(&t1), false, false).1.is_err(), "oldest evicted");
    for t in &newer {
        assert!(hello(&addr, "ann", "", Some(t), false, false).1.is_ok());
    }
}

#[test]
fn old_accounts_load_and_unused_tokens_expire() {
    let data = temp_dir("old");
    // accounts.json from before tokens (legacy sha256 password) and one with a stale session
    let legacy = sha256_hex("salt1:pw-old");
    let fresh = "ab".repeat(32);
    let stale = "cd".repeat(32);
    let now = proto::unix_now();
    let json = serde_json::json!({
        "old": { "salt": "salt1", "hash": legacy },
        "kim": { "salt": "salt2", "hash": sha256_hex("salt2:pw-kim"), "tokens": [
            { "sha256": sha256_hex(&fresh), "last": now - 86_400.0 },
            { "sha256": sha256_hex(&stale), "last": now - 100.0 * 86_400.0 },
        ] },
    });
    std::fs::write(data.join("accounts.json"), json.to_string()).unwrap();
    let addr = start(&data);
    // password login on the old account still works, and asking to stay mints a token
    let t = hello(&addr, "old", "pw-old", None, false, true).1.unwrap().unwrap();
    assert!(hello(&addr, "old", "", Some(&t), false, false).1.is_ok());
    // the password upgrade (argon2) kept the session
    assert!(std::fs::read_to_string(data.join("accounts.json")).unwrap().contains("argon2id"));
    assert!(hello(&addr, "old", "", Some(&t), false, false).1.is_ok());
    // a session unused for 90+ days is gone, a recent one works
    assert!(hello(&addr, "kim", "", Some(&stale), false, false).1.is_err());
    assert!(hello(&addr, "kim", "", Some(&fresh), false, false).1.is_ok());
}

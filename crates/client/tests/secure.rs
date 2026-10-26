//! The hardened server end to end: TLS + pinning through the real client `Net`, plaintext refusal,
//! sha256 → argon2 migration, invite codes, size/rate limits, login lockout.
use proto::{decode, encode, ClientMsg, ServerMsg};
use server::guard::Limits;
use server::{Config, Plain};
use space_rts::net::{Net, NetEvent};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("secure-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn start(cfg: Config) -> String {
    std::env::set_var("HOHMANN_PINS", std::env::temp_dir().join(format!("secure-pins-{}.txt", std::process::id())));
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    std::thread::spawn(move || server::serve_with(l, cfg).unwrap());
    // wait until it accepts (cert generation happens before the accept loop)
    let end = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(&addr).is_err() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
    addr
}

fn hello(user: &str, pass: &str, register: bool, invite: Option<&str>) -> ClientMsg {
    ClientMsg::Hello { user: user.into(), pass: pass.into(), register, invite: invite.map(String::from) }
}

/// Connect with the real client and return the first answer to Hello (Ok = Welcome user, Err = text).
fn login(addr: &str, h: ClientMsg) -> Result<String, String> {
    let net = Net::connect(addr.to_string(), h);
    let end = Instant::now() + Duration::from_secs(20);
    while Instant::now() < end {
        for e in net.poll() {
            match e {
                NetEvent::Msg(ServerMsg::Welcome { user }) => return Ok(user),
                NetEvent::Msg(ServerMsg::Error { msg }) => return Err(msg),
                NetEvent::Failed(m) => return Err(m),
                NetEvent::Closed => return Err("closed".into()),
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err("timeout".into())
}

fn cert_fingerprint(data: &std::path::Path) -> String {
    use rustls::pki_types::{pem::PemObject, CertificateDer};
    let c = CertificateDer::from_pem_file(data.join("tls/cert.pem")).unwrap();
    space_rts::tls::fingerprint(&c)
}

/// Plaintext line client (what old app builds and LAN tools speak).
fn plain(addr: &str) -> (TcpStream, BufReader<TcpStream>) {
    let s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    (s.try_clone().unwrap(), BufReader::new(s))
}

fn next(r: &mut BufReader<TcpStream>) -> Option<ServerMsg> {
    let mut line = String::new();
    match r.read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => decode(&line).ok(),
    }
}

/// Read until the server hangs up; returns the error texts seen.
fn errors_until_closed(r: &mut BufReader<TcpStream>) -> Vec<String> {
    let mut errs = vec![];
    let end = Instant::now() + Duration::from_secs(10);
    let mut line = String::new();
    while Instant::now() < end {
        line.clear();
        match r.read_line(&mut line) {
            Ok(0) => return errs,
            Ok(_) => {
                if let Ok(ServerMsg::Error { msg }) = decode(&line) {
                    errs.push(msg);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => continue,
            Err(_) => return errs,
        }
    }
    panic!("server did not close the connection; errors: {errs:?}");
}

#[test]
fn tls_login_and_pinning() {
    let data = temp_dir("tls");
    let addr = start(Config { plain: Plain::Never, ..Config::new(data.clone()) });
    assert_eq!(login(&addr, hello("ann", "pw", true, None)), Ok("ann".into()));
    let fp = cert_fingerprint(&data);
    assert_eq!(login(&format!("{addr}#{fp}"), hello("ann", "pw", false, None)), Ok("ann".into()), "right pin");
    let bad = login(&format!("{addr}#{}", "00".repeat(32)), hello("ann", "pw", false, None)).unwrap_err();
    assert!(bad.contains("certificate changed"), "{bad}");
    // a plaintext client gets told why, then the door closes
    let (mut w, mut r) = plain(&addr);
    w.write_all(encode(&hello("ann", "pw", false, None)).as_bytes()).unwrap();
    match next(&mut r) {
        Some(ServerMsg::Error { msg }) => assert!(msg.contains("updated app"), "{msg}"),
        m => panic!("expected an error, got {m:?}"),
    }
    assert!(next(&mut r).is_none());
}

#[test]
fn old_sha256_accounts_upgrade_to_argon2() {
    let data = temp_dir("migrate");
    // salted sha256 of "secret" with salt "abc", as the old server stored it
    let hash = {
        use sha2::{Digest, Sha256};
        Sha256::digest(b"abc:secret").iter().map(|b| format!("{b:02x}")).collect::<String>()
    };
    std::fs::write(data.join("accounts.json"), format!("{{\"old\":{{\"salt\":\"abc\",\"hash\":\"{hash}\"}}}}")).unwrap();
    let addr = start(Config::new(data.clone()));
    assert_eq!(login(&addr, hello("old", "wrong", false, None)), Err("wrong password".into()));
    assert_eq!(login(&addr, hello("old", "secret", false, None)), Ok("old".into()));
    let saved = std::fs::read_to_string(data.join("accounts.json")).unwrap();
    assert!(saved.contains("$argon2id$") && !saved.contains(&hash), "{saved}");
    assert_eq!(login(&addr, hello("old", "secret", false, None)), Ok("old".into()), "argon2 login");
    assert_eq!(login(&addr, hello("old", "wrong", false, None)), Err("wrong password".into()));
}

#[test]
fn invite_only_registration() {
    let data = temp_dir("invite");
    let inv = data.join("invites.txt");
    std::fs::write(&inv, "ALPHA\nBRAVO\n").unwrap();
    let addr = start(Config { invites: Some(inv.clone()), ..Config::new(data) });
    assert!(login(&addr, hello("ann", "pw", true, None)).unwrap_err().contains("invite"));
    assert!(login(&addr, hello("ann", "pw", true, Some("NOPE"))).unwrap_err().contains("isn't valid"));
    assert_eq!(login(&addr, hello("ann", "pw", true, Some("ALPHA"))), Ok("ann".into()));
    assert_eq!(std::fs::read_to_string(&inv).unwrap().trim(), "BRAVO");
    assert!(login(&addr, hello("bob", "pw", true, Some("ALPHA"))).unwrap_err().contains("isn't valid"), "codes work once");
    assert_eq!(login(&addr, hello("ann", "pw", false, None)), Ok("ann".into()), "existing accounts log in without a code");
    assert!(login(&addr, hello("Bot 1 (normal)", "pw", true, Some("BRAVO"))).unwrap_err().contains("kept for bots"));
}

#[test]
fn oversized_lines_and_floods_are_cut_off() {
    let addr = start(Config::new(temp_dir("limits")));
    let (mut w, mut r) = plain(&addr);
    let _ = w.write_all(&vec![b'x'; proto::wire::MAX_CLIENT_LINE + 10]);
    let errs = errors_until_closed(&mut r);
    assert!(errs.iter().any(|e| e.contains("too long")), "{errs:?}");

    let (mut w, mut r) = plain(&addr);
    let flood: String = (0..400).map(|_| encode(&ClientMsg::Ping)).collect();
    let _ = w.write_all(flood.as_bytes());
    let errs = errors_until_closed(&mut r);
    assert!(errs.iter().any(|e| e.contains("slow down")), "{errs:?}");

    // a NaN-ish / absurd message is refused, the connection stays up
    let (mut w, mut r) = plain(&addr);
    w.write_all(encode(&hello(&"u".repeat(100), "pw", true, None)).as_bytes()).unwrap();
    assert!(matches!(next(&mut r), Some(ServerMsg::Error { msg }) if msg.contains("out of range")));
    w.write_all(encode(&ClientMsg::Ping).as_bytes()).unwrap();
    assert!(matches!(next(&mut r), Some(ServerMsg::Pong)));
}

#[test]
fn wrong_passwords_lock_the_account_for_a_while() {
    let limits = Limits { account_fails: (3, Duration::from_secs(600)), ..Limits::default() };
    let addr = start(Config { limits, ..Config::new(temp_dir("lockout")) });
    assert_eq!(login(&addr, hello("ann", "pw", true, None)), Ok("ann".into()));
    for _ in 0..3 {
        assert_eq!(login(&addr, hello("ann", "guess", false, None)), Err("wrong password".into()));
    }
    let e = login(&addr, hello("ann", "pw", false, None)).unwrap_err();
    assert!(e.contains("too many wrong passwords"), "{e}");
}

#[test]
fn per_ip_connection_cap() {
    let limits = Limits { max_conns_per_ip: 3, ..Limits::default() };
    let addr = start(Config { limits, ..Config::new(temp_dir("cap")) });
    let keep: Vec<_> = (0..3).map(|_| plain(&addr)).collect();
    std::thread::sleep(Duration::from_millis(200));
    let (_w, mut r) = plain(&addr);
    assert!(next(&mut r).is_none(), "4th connection from one IP is closed");
    drop(keep);
    std::thread::sleep(Duration::from_millis(300));
    let (mut w, mut r) = plain(&addr);
    w.write_all(encode(&ClientMsg::Ping).as_bytes()).unwrap();
    assert!(matches!(next(&mut r), Some(ServerMsg::Pong)), "slots come back");
}

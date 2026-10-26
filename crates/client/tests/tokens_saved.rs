//! The real client backend with session tokens: it saves the token, not the password, logs back in
//! with it and revokes it on logout. Its own test binary because it sets HOME for the process.
use proto::{decode, encode, ClientMsg, ServerMsg};
use server::Config;
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


/// The real client backend saves the token, not the password, and logs back in with it.
#[test]
fn saved_session_keeps_the_token_not_the_password() {
    let data = temp_dir("saved");
    let addr = start(&data);
    let home = temp_dir("home");
    std::env::set_var("HOME", &home);
    std::env::set_var("HOHMANN_PINS", home.join("pins.txt"));
    let (_c, _) = hello(&addr, "sam", "pw-sam-secret", None, true, false);
    use ui::Backend;
    let mut be = space_rts::backend::ClientBackend::new(addr.clone());
    be.act(ui::UiAction::Login { server: addr.clone(), user: "sam".into(), pass: "pw-sam-secret".into(), register: false, invite: String::new() });
    let end = Instant::now() + Duration::from_secs(20);
    let logged_in = |be: &mut space_rts::backend::ClientBackend| be.poll().iter().any(|e| matches!(e, ui::BackendEvent::LoggedIn { .. }));
    while !logged_in(&mut be) {
        assert!(Instant::now() < end, "login timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
    let saved = std::fs::read_to_string(home.join(".config/hohmanns-reach/session.json")).unwrap();
    assert!(!saved.contains("pw-sam-secret") && saved.contains("\"token\""), "{saved}");
    // a new launch resumes from the saved token
    let mut be2 = space_rts::backend::ClientBackend::new(addr.clone());
    be2.resume();
    let end = Instant::now() + Duration::from_secs(20);
    while !logged_in(&mut be2) {
        assert!(Instant::now() < end, "resume timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
    // logging out revokes it on the server
    be2.act(ui::UiAction::Logout);
    let token = serde_json::from_str::<serde_json::Value>(&saved).unwrap()["token"].as_str().unwrap().to_string();
    // the Logout goes out on the connection's writer thread; wait until the server has handled it
    let end = Instant::now() + Duration::from_secs(5);
    while hello(&addr, "sam", "", Some(&token), false, false).1.is_ok() {
        assert!(Instant::now() < end, "token still valid after logout");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!home.join(".config/hohmanns-reach/session.json").exists());
}

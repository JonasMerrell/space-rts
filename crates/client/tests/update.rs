//! "New version available": the server announces the newest client version in the Welcome
//! (`--latest-client`, else the deployed browser build's version.json) and the app compares it.
use proto::{decode, encode, ClientMsg, ServerMsg};
use server::Config;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("update-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn start(cfg: Config) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    std::thread::spawn(move || server::serve_with(l, cfg).unwrap());
    let end = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(&addr).is_err() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
    addr
}

/// Register `user` over a plaintext (loopback) connection and return the Welcome's latest_client.
fn latest(addr: &str, user: &str) -> Option<String> {
    welcome(addr, user).0
}

/// The Welcome's (latest_client, update_url).
fn welcome(addr: &str, user: &str) -> (Option<String>, Option<String>) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let h = ClientMsg::Hello { user: user.into(), pass: "pw-123456".into(), register: true, invite: None, token: None, stay: false };
    s.write_all(encode(&h).as_bytes()).unwrap();
    let mut r = BufReader::new(s);
    let mut line = String::new();
    loop {
        line.clear();
        assert!(r.read_line(&mut line).unwrap() > 0, "closed before Welcome");
        match decode::<ServerMsg>(&line) {
            Ok(ServerMsg::Welcome { latest_client, update_url, .. }) => return (latest_client, update_url),
            Ok(ServerMsg::Error { msg }) => panic!("{msg}"),
            _ => {}
        }
    }
}

fn write_version(data: &Path, json: &str) {
    std::fs::create_dir_all(data.join("public/web")).unwrap();
    std::fs::write(data.join("public/web/version.json"), json).unwrap();
}

#[test]
fn welcome_announces_the_deployed_web_version() {
    let data = temp_dir("web");
    let addr = start(Config::new(data.clone()));
    // nothing deployed: nothing announced, nobody is nagged
    assert_eq!(latest(&addr, "ann"), None);
    // a web release lands (no restart needed)
    write_version(&data, r#"{"version":"0.9.1"}"#);
    assert_eq!(latest(&addr, "bob").as_deref(), Some("0.9.1"));
    // junk in the file is ignored rather than passed on
    write_version(&data, r#"{"version":"<script>"}"#);
    assert_eq!(latest(&addr, "cy"), None);
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn latest_client_flag_wins_over_the_web_build() {
    let data = temp_dir("flag");
    write_version(&data, r#"{"version":"0.9.1"}"#);
    let mut cfg = Config::new(data.clone());
    cfg.latest_client = Some("v1.0.0".into());
    let addr = start(cfg);
    let v = latest(&addr, "dee").unwrap();
    assert_eq!(v, "1.0.0");
    // no --update-url: nothing sent, the app opens its built-in download page
    assert_eq!(welcome(&addr, "eve").1, None);
    assert_eq!(space_rts::update::download_url(None), "https://hohmannsreach.duckdns.org:7777/");
    // and the app's own comparison
    let u = space_rts::update::update_for(Some(&v), Some("0.8.2")).unwrap();
    assert_eq!((u.latest.as_str(), u.current.as_str()), ("1.0.0", "0.8.2"));
    assert!(space_rts::update::update_for(Some(&v), Some("1.0.0")).is_none());
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn update_url_flag_is_sent_to_apps() {
    let data = temp_dir("url");
    let mut cfg = Config::new(data.clone());
    cfg.latest_client = Some("0.9.0".into());
    cfg.update_url = Some("https://game.example:7777/".into());
    let addr = start(cfg);
    let (v, url) = welcome(&addr, "fay");
    assert_eq!(v.as_deref(), Some("0.9.0"));
    assert_eq!(space_rts::update::download_url(url.as_deref()), "https://game.example:7777/");
    let _ = std::fs::remove_dir_all(&data);
}

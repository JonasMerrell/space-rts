//! Saved session: handed to the Android background poller (NotifyJob.java), which checks the server
//! for fleet arrivals, attacks and captures every ~15 min while the app is closed, and read back on
//! launch so the player stays logged in until they log out.
use std::path::PathBuf;

fn file() -> Option<PathBuf> {
    // Android: the app's private files dir (only the app can read it); desktop: the config dir
    if cfg!(target_os = "android") {
        Some(PathBuf::from("/data/data/net.lego.hohmannsreach/files/notify.json"))
    } else {
        std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config/hohmanns-reach/session.json"))
    }
}

/// (server, user, pass) from the last successful login, if the player hasn't logged out since.
pub fn load() -> Option<(String, String, String)> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(file()?).ok()?).ok()?;
    let get = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string).filter(|s| !s.is_empty());
    Some((get("server")?, get("user")?, get("pass")?))
}

pub fn save(server: &str, user: &str, pass: &str) {
    let Some(p) = file() else { return };
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, serde_json::json!({ "server": server, "user": user, "pass": pass }).to_string());
}

pub fn clear() {
    if let Some(p) = file() {
        let _ = std::fs::remove_file(p);
    }
}

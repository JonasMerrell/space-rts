//! Hand the login to the Android background poller (NotifyJob.java), which checks the server for
//! fleet arrivals, attacks and captures every ~15 min while the app is closed.
use std::path::PathBuf;

fn file() -> Option<PathBuf> {
    // the app's private files dir (only the app can read it); nothing to do on desktop
    cfg!(target_os = "android").then(|| PathBuf::from("/data/data/net.lego.hahmannsreach/files/notify.json"))
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

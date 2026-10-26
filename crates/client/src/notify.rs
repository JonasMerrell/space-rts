//! Saved session: handed to the Android background poller (NotifyJob.java), which checks the server
//! for fleet arrivals, attacks and captures every ~15 min while the app is closed, and read back on
//! launch so the player stays logged in until they log out. In the browser it lives in localStorage
//! and also turns on notifications for the hidden tab (web/hohmann.js).
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

#[cfg(not(target_arch = "wasm32"))]
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
    let text = read()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let get = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string).filter(|s| !s.is_empty());
    Some((get("server")?, get("user")?, get("pass")?))
}

#[cfg(target_arch = "wasm32")]
const KEY: &str = "hohmann.session";

#[cfg(target_arch = "wasm32")]
fn read() -> Option<String> {
    crate::web::store_get(KEY)
}

#[cfg(target_arch = "wasm32")]
pub fn save(server: &str, user: &str, pass: &str) {
    crate::web::store_set(KEY, &serde_json::json!({ "server": server, "user": user, "pass": pass }).to_string());
    crate::web::notifications(true);
}

#[cfg(target_arch = "wasm32")]
pub fn clear() {
    crate::web::store_remove(KEY);
    crate::web::notifications(false);
}

#[cfg(not(target_arch = "wasm32"))]
fn read() -> Option<String> {
    let p = file()?;
    // desktop: pick up a session saved before the rename (Android can't: the old package's files are private)
    let text = std::fs::read_to_string(&p).ok().or_else(|| {
        let old = p.to_string_lossy().replace("hohmanns-reach", "hahmanns-reach");
        std::fs::read_to_string(old).ok()
    })?;
    Some(text)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn save(server: &str, user: &str, pass: &str) {
    let Some(p) = file() else { return };
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, serde_json::json!({ "server": server, "user": user, "pass": pass }).to_string());
}

#[cfg(not(target_arch = "wasm32"))]
pub fn clear() {
    if let Some(p) = file() {
        let _ = std::fs::remove_file(p);
    }
}

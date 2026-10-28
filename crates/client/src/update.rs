//! "A new version is out": this build's version, the comparison with what the server announces,
//! and getting the new build (download page on phones/desktop, a page reload in the browser).
use ui::model::AppUpdate;

/// Where phones get the newest APK.
pub const DOWNLOAD_URL: &str = "https://github.com/JonasMerrell/space-rts/releases/tag/apk-latest";

/// The release version, baked in by scripts/release-apk.sh and scripts/release-web.sh
/// (`HOHMANN_VERSION=0.8.3`). Dev builds have none and never ask to update.
pub const APP_VERSION: Option<&str> = option_env!("HOHMANN_VERSION");

/// What the update card shows when the server's `latest` is newer than `mine`.
pub fn update_for(latest: Option<&str>, mine: Option<&str>) -> Option<AppUpdate> {
    let (latest, mine) = (latest?, mine?);
    proto::is_newer(latest, mine).then(|| AppUpdate {
        latest: latest.trim_start_matches('v').to_string(),
        current: mine.trim_start_matches('v').to_string(),
        reload: cfg!(target_arch = "wasm32"),
    })
}

/// The card's UPDATE / RELOAD button.
pub fn get(latest: &str) {
    #[cfg(target_arch = "wasm32")]
    crate::web::reload(latest);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = latest;
        open_url(DOWNLOAD_URL);
    }
}

#[cfg(target_os = "android")]
fn open_url(url: &str) {
    use macroquad::miniquad::native::android::{attach_jni_env, ACTIVITY};
    let Ok(url) = std::ffi::CString::new(url) else { return };
    unsafe {
        let env = attach_jni_env();
        let new_string_utf = (**env).NewStringUTF.unwrap();
        let jurl = new_string_utf(env, url.as_ptr());
        macroquad::miniquad::call_void_method!(env, ACTIVITY, "openUrl", "(Ljava/lang/String;)V", jurl);
    }
}

#[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
fn open_url(url: &str) {
    let cmd = if cfg!(windows) {
        std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    if let Err(e) = cmd {
        eprintln!("can't open {url}: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_newer_releases_ask() {
        let u = update_for(Some("0.8.3"), Some("0.8.2")).unwrap();
        assert_eq!((u.latest.as_str(), u.current.as_str(), u.reload), ("0.8.3", "0.8.2", false));
        assert_eq!(update_for(Some("v0.10.0"), Some("0.9.9")).unwrap().latest, "0.10.0");
        assert!(update_for(Some("0.8.2"), Some("0.8.2")).is_none());
        assert!(update_for(Some("0.8.1"), Some("0.8.2")).is_none());
        // old server (nothing announced) or a dev build (no version)
        assert!(update_for(None, Some("0.8.2")).is_none());
        assert!(update_for(Some("0.9.0"), None).is_none());
        assert!(update_for(Some("junk"), Some("0.8.2")).is_none());
    }
}

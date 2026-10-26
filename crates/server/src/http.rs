//! Tiny read-only HTTP/1.1 endpoint on the game port, so players can download the app from the
//! server itself: `GET /` (download page) and `GET /hohmann-reach.apk` (from `<data>/public/`).
//! Anything else is 404. Plain HTTP only; the APK's sha256 is shown on the page.
use crate::guard::{self, Bucket};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, TcpStream};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

pub const APK_NAME: &str = "hohmann-reach.apk";
const MAX_HEAD: usize = 4096;
/// Requests per minute per IP (page + download).
const RATE_PER_MIN: f64 = 12.0;
const BURST: f64 = 6.0;

static BUCKETS: Mutex<Option<HashMap<IpAddr, Bucket>>> = Mutex::new(None);

fn allowed(ip: IpAddr) -> bool {
    let mut g = BUCKETS.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    if map.len() > 10_000 {
        map.clear();
    }
    map.entry(ip).or_insert_with(|| Bucket::new(BURST)).take(RATE_PER_MIN / 60.0, BURST)
}

/// Does a connection start like an HTTP request we serve?
pub fn looks_like_http(first: &[u8]) -> bool {
    first.starts_with(b"GET ") || first.starts_with(b"HEAD")
}

fn respond(s: &mut TcpStream, status: &str, ctype: &str, extra: &str, body: &[u8], head_only: bool) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n{extra}\r\n",
        body.len()
    );
    let _ = s.write_all(head.as_bytes());
    if !head_only {
        let _ = s.write_all(body);
    }
}

fn page(sha: Option<&str>, size: Option<u64>) -> String {
    let info = match (sha, size) {
        (Some(h), Some(n)) => format!("<p>{:.1} MB &middot; sha256 <code>{h}</code></p>", n as f64 / 1e6),
        (None, Some(n)) => format!("<p>{:.1} MB</p>", n as f64 / 1e6),
        _ => "<p>No app build has been published on this server yet.</p>".to_string(),
    };
    format!(
        "<!doctype html><html><head><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\">\
<title>Hohmann's Reach</title><style>body{{font-family:sans-serif;background:#1b1340;color:#eee;margin:0;padding:24px;max-width:640px}}\
a.b{{display:inline-block;background:#ffd23f;color:#111;padding:14px 22px;border-radius:10px;font-weight:bold;text-decoration:none}}\
code{{word-break:break-all;font-size:12px;color:#bbb}}</style></head><body><h1>Hohmann's Reach</h1>\
<p>Multiplayer orbital strategy for Android.</p><p><a class=b href=\"/{APK_NAME}\">Download the app</a></p>{info}\
<p>Install it, open it, and use this server's address on the login screen.</p></body></html>"
    )
}

/// Serve one HTTP request on `s` (the bytes are still unread). Closes the connection after.
pub fn serve(mut s: TcpStream, ip: IpAddr, data: &Path) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(120)));
    // read the request head (request line + headers), capped
    let mut buf = Vec::with_capacity(512);
    let mut chunk = [0u8; 512];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && !buf.windows(2).any(|w| w == b"\n\n") {
        if buf.len() > MAX_HEAD {
            respond(&mut s, "431 Request Header Fields Too Large", "text/plain", "", b"too large\n", false);
            return;
        }
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    if !allowed(ip) {
        guard::log_limited(ip);
        respond(&mut s, "429 Too Many Requests", "text/plain", "Retry-After: 60\r\n", b"slow down\n", false);
        return;
    }
    let line = String::from_utf8_lossy(&buf[..buf.iter().position(|&b| b == b'\n').unwrap_or(buf.len())]).to_string();
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("").split('?').next().unwrap_or("");
    let head_only = method == "HEAD";
    if method != "GET" && !head_only {
        respond(&mut s, "405 Method Not Allowed", "text/plain", "Allow: GET, HEAD\r\n", b"method not allowed\n", false);
        return;
    }
    let dir = data.join("public");
    let apk = dir.join(APK_NAME);
    match path {
        "/" | "/index.html" => {
            let sha = std::fs::read_to_string(dir.join(format!("{APK_NAME}.sha256"))).ok();
            let sha = sha.as_deref().and_then(|t| t.split_whitespace().next()).filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()));
            let size = std::fs::metadata(&apk).ok().map(|m| m.len());
            respond(&mut s, "200 OK", "text/html; charset=utf-8", "Cache-Control: no-cache\r\n", page(sha, size).as_bytes(), head_only);
        }
        p if p == format!("/{APK_NAME}") => match std::fs::File::open(&apk) {
            Ok(mut f) => {
                let len = f.metadata().map(|m| m.len()).unwrap_or(0);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.android.package-archive\r\nContent-Length: {len}\r\n\
Content-Disposition: attachment; filename=\"{APK_NAME}\"\r\nConnection: close\r\n\r\n"
                );
                if s.write_all(head.as_bytes()).is_ok() && !head_only {
                    let _ = std::io::copy(&mut f, &mut s);
                }
            }
            Err(_) => respond(&mut s, "404 Not Found", "text/plain", "", b"no app published yet\n", head_only),
        },
        _ => respond(&mut s, "404 Not Found", "text/plain", "", b"not found\n", head_only),
    }
    let _ = s.flush();
    let _ = s.shutdown(std::net::Shutdown::Write);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn get(port: u16, req: &str) -> String {
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.write_all(req.as_bytes()).unwrap();
        let mut out = String::new();
        let _ = c.read_to_string(&mut out);
        out
    }

    #[test]
    fn serves_page_apk_and_404() {
        let dir = std::env::temp_dir().join(format!("httptest-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("public")).unwrap();
        std::fs::write(dir.join("public").join(APK_NAME), b"APKDATA").unwrap();
        std::fs::write(dir.join("public").join(format!("{APK_NAME}.sha256")), format!("{}  {APK_NAME}\n", "ab".repeat(32))).unwrap();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let d = dir.clone();
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let ip = s.peer_addr().unwrap().ip();
                serve(s, ip, &d);
            }
        });
        let page = get(port, "GET / HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(page.starts_with("HTTP/1.1 200") && page.contains(&"ab".repeat(32)), "{page}");
        let apk = get(port, &format!("GET /{APK_NAME} HTTP/1.1\r\n\r\n"));
        assert!(apk.ends_with("APKDATA") && apk.contains("android.package-archive"), "{apk}");
        assert!(get(port, "GET /../accounts.json HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 404"));
        assert!(get(port, "POST / HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 405"));
        assert!(get(port, &format!("GET / HTTP/1.1\r\n{}\r\n\r\n", "X: y\r\n".repeat(1000))).starts_with("HTTP/1.1 431"));
        assert!(looks_like_http(b"GET ") && !looks_like_http(b"{\"He"));
    }
}

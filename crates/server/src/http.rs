//! Small read-only HTTP/1.1 endpoint on the game port, over plain TCP or inside TLS:
//! - `GET /` download page, `GET /hohmann-reach.apk` (from `<data>/public/`), plain-HTTP friendly;
//! - `GET /play/...` the browser build, from a file map built at startup from `<data>/public/web/`
//!   (request paths are never joined onto the filesystem), keep-alive so its ~300 files load fast;
//! - `GET /ws` + `Upgrade: websocket` hands the connection to the game protocol (see `ws.rs`).
//!
//! GET/HEAD only, no request bodies; anything malformed closes the connection.
use crate::guard::{self, Bucket};
use std::collections::HashMap;
use std::io::{BufRead, Read, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, UNIX_EPOCH};

pub const APK_NAME: &str = "hohmann-reach.apk";
const MAX_HEAD: usize = 8192;
const MAX_HEADERS: usize = 50;
/// Per-IP budgets (tokens per minute, burst), so one address can't eat the home upload.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Kind {
    /// download page, /ws handshakes, 404s: one token per request
    Page,
    /// the APK (~20 MB): one token per download
    Apk,
    /// the browser build: one token per file plus one per 32 KiB, so a full first load
    /// (~300 files, ~23 MB) fits the burst and the sustained rate stays near 1.3 Mbit/s
    Play,
}

impl Kind {
    fn budget(self) -> (f64, f64) {
        match self {
            Kind::Page => (12.0, 6.0),
            Kind::Apk => (3.0, 3.0),
            Kind::Play => (300.0, 1200.0),
        }
    }
}

/// Bytes per extra /play/ token.
const PLAY_TOKEN_BYTES: u64 = 32 * 1024;
/// Requests served on one keep-alive connection.
const MAX_KEEPALIVE: usize = 1000;

static BUCKETS: Mutex<Option<HashMap<(IpAddr, Kind), Bucket>>> = Mutex::new(None);

fn allowed(ip: IpAddr, kind: Kind, cost: f64) -> bool {
    // loopback = tests and tools on the server itself
    if ip.is_loopback() {
        return true;
    }
    let (per_min, burst) = kind.budget();
    let mut g = BUCKETS.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    if map.len() > 10_000 {
        // forget only addresses that have been quiet long enough to be back at full budget
        map.retain(|(_, k), b| {
            let (r, bu) = k.budget();
            !b.is_full(r / 60.0, bu)
        });
    }
    map.entry((ip, kind)).or_insert_with(|| Bucket::new(burst)).take_n(cost, per_min / 60.0, burst)
}

/// Does a connection start like an HTTP request we serve?
pub fn looks_like_http(first: &[u8]) -> bool {
    matches!(first.first(), Some(b'G' | b'H'))
}

pub struct Request {
    pub method: String,
    pub path: String,
    headers: Vec<(String, String)>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    fn has_token(&self, name: &str, token: &str) -> bool {
        self.header(name).is_some_and(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case(token)))
    }
    /// A WebSocket handshake for the game protocol?
    pub fn is_websocket(&self) -> bool {
        self.method == "GET"
            && self.path == "/ws"
            && self.has_token("upgrade", "websocket")
            && self.has_token("connection", "upgrade")
            && self.header("sec-websocket-version") == Some("13")
            && self.header("sec-websocket-key").is_some_and(|k| k.len() == 24)
    }
}

/// Read one request head (request line + headers). None on EOF, timeout, or anything malformed,
/// oversized or slower than `deadline` (the caller then just closes the connection).
pub fn read_head<R: BufRead>(r: &mut R, deadline: Duration) -> Option<Request> {
    let start = Instant::now();
    let mut buf: Vec<u8> = Vec::with_capacity(512);
    loop {
        if start.elapsed() > deadline {
            return None;
        }
        let avail = r.fill_buf().ok()?;
        if avail.is_empty() {
            return None;
        }
        // stop exactly at the end of the head: a keep-alive connection's next request stays unread
        let mut take = avail.len();
        let mut done = false;
        for (i, &b) in avail.iter().enumerate() {
            buf.push(b);
            if b == b'\n' && (buf.ends_with(b"\r\n\r\n") || buf.ends_with(b"\n\n")) {
                take = i + 1;
                done = true;
                break;
            }
            if buf.len() > MAX_HEAD {
                return None;
            }
        }
        r.consume(take);
        if done {
            break;
        }
    }
    let text = std::str::from_utf8(&buf).ok()?;
    let mut lines = text.lines();
    let mut parts = lines.next()?.split(' ');
    let (method, path, version) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || !version.starts_with("HTTP/1.") || !path.starts_with('/') {
        return None;
    }
    let mut headers = vec![];
    for l in lines.take_while(|l| !l.is_empty()) {
        let (k, v) = l.split_once(':')?;
        if k.is_empty() || k.contains(' ') || headers.len() >= MAX_HEADERS {
            return None;
        }
        headers.push((k.to_string(), v.trim().to_string()));
    }
    let req = Request { method: method.to_string(), path: path.split('?').next().unwrap_or("").to_string(), headers };
    // no request bodies anywhere
    if req.header("content-length").is_some_and(|v| v != "0") || req.header("transfer-encoding").is_some() {
        return None;
    }
    Some(req)
}

/// One file of the browser build.
#[derive(Debug)]
struct WebFile {
    path: PathBuf,
    ctype: &'static str,
    etag: String,
}

/// The browser build's files, by URL path under /play/, fixed when the server starts.
#[derive(Debug, Default)]
pub struct Site {
    files: HashMap<String, WebFile>,
}

fn content_type(name: &str) -> Option<&'static str> {
    Some(match name.rsplit_once('.')?.1 {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "wasm" => "application/wasm",
        "png" => "image/png",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "json" => "application/json",
        "webmanifest" => "application/manifest+json",
        "ttf" => "font/ttf",
        _ => return None,
    })
}

impl Site {
    /// Every servable file under `dir` (known extensions, no dotfiles, no symlinks).
    pub fn scan(dir: &Path) -> Site {
        let mut files = HashMap::new();
        let mut todo = vec![(dir.to_path_buf(), String::new())];
        while let Some((d, prefix)) = todo.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                let Ok(ft) = e.file_type() else { continue };
                if name.starts_with('.') || ft.is_symlink() || !name.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) {
                    continue;
                }
                let url = format!("{prefix}/{name}");
                if ft.is_dir() {
                    todo.push((e.path(), url));
                } else if let (Some(ctype), Ok(meta)) = (content_type(&name), e.metadata()) {
                    let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
                    files.insert(url, WebFile { path: e.path(), ctype, etag: format!("\"{:x}-{mtime:x}\"", meta.len()) });
                }
            }
        }
        Site { files }
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// The browser build, rescanned when `<data>/public/web` changes, so a web-only release needs no
/// server restart (checked at most every 10 s; the stamp is the newest mtime of the dir and its top files).
#[derive(Debug)]
pub struct LiveSite {
    dir: std::path::PathBuf,
    cur: std::sync::Mutex<(std::sync::Arc<Site>, Option<std::time::SystemTime>, std::time::Instant)>,
}

fn stamp(dir: &Path) -> Option<std::time::SystemTime> {
    let mut newest = std::fs::metadata(dir).and_then(|m| m.modified()).ok()?;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        if let Ok(t) = e.metadata().and_then(|m| m.modified()) {
            newest = newest.max(t);
        }
    }
    Some(newest)
}

impl LiveSite {
    pub fn new(dir: std::path::PathBuf) -> LiveSite {
        let site = std::sync::Arc::new(Site::scan(&dir));
        let st = stamp(&dir);
        LiveSite { dir, cur: std::sync::Mutex::new((site, st, std::time::Instant::now())) }
    }

    pub fn current(&self) -> std::sync::Arc<Site> {
        let mut g = self.cur.lock().unwrap_or_else(|e| e.into_inner());
        if g.2.elapsed() >= Duration::from_secs(10) {
            g.2 = std::time::Instant::now();
            let st = stamp(&self.dir);
            if st != g.1 {
                g.0 = std::sync::Arc::new(Site::scan(&self.dir));
                g.1 = st;
                eprintln!("browser build reloaded from {} ({} files)", self.dir.display(), g.0.files.len());
            }
        }
        g.0.clone()
    }
}

impl Default for LiveSite {
    fn default() -> LiveSite {
        LiveSite::new(std::path::PathBuf::from("/nonexistent"))
    }
}

/// What the server knows when answering one HTTP connection.
pub struct Ctx<'a> {
    pub ip: IpAddr,
    pub data: &'a Path,
    pub site: &'a Site,
    /// inside TLS
    pub secure: bool,
    /// the game protocol may run over this connection (TLS, or plaintext the server allows)
    pub game_ok: bool,
    /// https://host[:port] origins the browser build is served from (the web cert's names)
    pub origins: &'a [String],
    /// plain /play/ requests from outside get sent here, when set
    pub https_base: Option<&'a str>,
    pub head_timeout: Duration,
}

/// A browser page may open the game socket only from our own pages.
fn origin_ok(origin: Option<&str>, ctx: &Ctx) -> bool {
    let Some(o) = origin else { return true }; // not a browser
    if ctx.origins.iter().any(|a| a.eq_ignore_ascii_case(o)) {
        return true;
    }
    // the LAN page over plain http (http://192.168.x.y:7777, http://localhost:7777)
    !ctx.secure
        && o.strip_prefix("http://")
            .map(|h| h.rsplit_once(':').filter(|(_, p)| p.parse::<u16>().is_ok()).map_or(h, |(h, _)| h))
            .is_some_and(|h| h == "localhost" || h.parse::<IpAddr>().is_ok_and(crate::is_lan))
}

fn respond(s: &mut dyn Write, status: &str, ctype: &str, extra: &str, body: &[u8], head_only: bool, keep: bool) -> bool {
    let conn = if keep { "keep-alive" } else { "close" };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: {conn}\r\nX-Content-Type-Options: nosniff\r\n{extra}\r\n",
        body.len()
    );
    s.write_all(head.as_bytes()).is_ok() && (head_only || s.write_all(body).is_ok()) && s.flush().is_ok()
}

fn page(sha: Option<&str>, size: Option<u64>, play: bool) -> String {
    let info = match (sha, size) {
        (Some(h), Some(n)) => format!("<p>{:.1} MB &middot; sha256 <code>{h}</code></p>", n as f64 / 1e6),
        (None, Some(n)) => format!("<p>{:.1} MB</p>", n as f64 / 1e6),
        _ => "<p>No app build has been published on this server yet.</p>".to_string(),
    };
    let play = if play { "<p><a class=b href=\"/play/\">Play in your browser</a></p><p>Phone or computer, no install needed. Same account as the app.</p>" } else { "" };
    format!(
        "<!doctype html><html><head><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\">\
<title>Hohmann's Reach</title><style>body{{font-family:sans-serif;background:#1b1340;color:#eee;margin:0;padding:24px;max-width:640px}}\
a.b{{display:inline-block;background:#ffd23f;color:#111;padding:14px 22px;border-radius:10px;font-weight:bold;text-decoration:none}}\
code{{word-break:break-all;font-size:12px;color:#bbb}}</style></head><body><h1>Hohmann's Reach</h1>\
<p>Multiplayer orbital strategy.</p>{play}<h2>Android app</h2><p><a class=b href=\"/{APK_NAME}\">Download the app</a></p>{info}\
<p>Install it, open it, and use this server's address on the login screen.</p></body></html>"
    )
}

/// What to do with the connection after `serve`.
pub enum Outcome {
    Close,
    /// a WebSocket handshake was accepted (101 sent): run the game protocol on it
    WebSocket,
}

/// Answer HTTP requests on a connection whose first bytes are still unread in `r`.
pub fn serve<R: BufRead>(r: &mut R, w: &mut dyn Write, ctx: &Ctx) -> Outcome {
    for _ in 0..MAX_KEEPALIVE {
        let Some(req) = read_head(r, ctx.head_timeout) else { return Outcome::Close };
        match answer(&req, w, ctx) {
            Next::KeepAlive => continue,
            Next::Close => return Outcome::Close,
            Next::WebSocket => return Outcome::WebSocket,
        }
    }
    Outcome::Close
}

enum Next {
    KeepAlive,
    Close,
    WebSocket,
}

fn answer(req: &Request, w: &mut dyn Write, ctx: &Ctx) -> Next {
    let path = req.path.as_str();
    let play = path == "/play" || path.starts_with("/play/");
    let (kind, cost) = if play {
        let size = ctx.site.files.get(path.strip_prefix("/play").unwrap_or("")).and_then(|f| std::fs::metadata(&f.path).ok()).map_or(0, |m| m.len());
        (Kind::Play, 1.0 + (size / PLAY_TOKEN_BYTES) as f64)
    } else if path == format!("/{APK_NAME}") {
        (Kind::Apk, 1.0)
    } else {
        (Kind::Page, 1.0)
    };
    if !allowed(ctx.ip, kind, cost) {
        guard::log_limited(ctx.ip);
        respond(w, "429 Too Many Requests", "text/plain", "Retry-After: 60\r\n", b"slow down\n", false, false);
        return Next::Close;
    }
    let head_only = req.method == "HEAD";
    if req.method != "GET" && !head_only {
        respond(w, "405 Method Not Allowed", "text/plain", "Allow: GET, HEAD\r\n", b"method not allowed\n", false, false);
        return Next::Close;
    }
    let lower = path.to_ascii_lowercase();
    if path.contains("..") || lower.contains("%2e") || lower.contains("%2f") || lower.contains("%5c") || path.contains('\\') || path.contains('\0') || lower.contains("%00") {
        respond(w, "400 Bad Request", "text/plain", "", b"bad path\n", head_only, false);
        return Next::Close;
    }
    if path == "/ws" {
        return websocket(req, w, ctx);
    }
    let dir = ctx.data.join("public");
    let apk = dir.join(APK_NAME);
    match path {
        "/" | "/index.html" => {
            let sha = std::fs::read_to_string(dir.join(format!("{APK_NAME}.sha256"))).ok();
            let sha = sha.as_deref().and_then(|t| t.split_whitespace().next()).filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()));
            let size = std::fs::metadata(&apk).ok().map(|m| m.len());
            let body = page(sha, size, !ctx.site.is_empty());
            respond(w, "200 OK", "text/html; charset=utf-8", "Cache-Control: no-cache\r\n", body.as_bytes(), head_only, false);
        }
        p if p == format!("/{APK_NAME}") => match std::fs::File::open(&apk) {
            Ok(mut f) => {
                let len = f.metadata().map(|m| m.len()).unwrap_or(0);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.android.package-archive\r\nContent-Length: {len}\r\n\
Content-Disposition: attachment; filename=\"{APK_NAME}\"\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n\r\n"
                );
                if w.write_all(head.as_bytes()).is_ok() && !head_only {
                    let _ = std::io::copy(&mut f, w);
                }
                let _ = w.flush();
            }
            Err(_) => {
                respond(w, "404 Not Found", "text/plain", "", b"no app published yet\n", head_only, false);
            }
        },
        _ if play => return play_file(req, w, ctx, head_only),
        _ => {
            respond(w, "404 Not Found", "text/plain", "", b"not found\n", head_only, false);
        }
    }
    Next::Close
}

fn play_file(req: &Request, w: &mut dyn Write, ctx: &Ctx, head_only: bool) -> Next {
    // the browser build from outside the LAN must come over https (it can't log in otherwise)
    if let (false, Some(base)) = (ctx.secure, ctx.https_base) {
        if !crate::is_lan(ctx.ip) {
            let loc = format!("Location: {base}/play/\r\n");
            respond(w, "301 Moved Permanently", "text/plain", &loc, b"use https\n", head_only, false);
            return Next::Close;
        }
    }
    let rel = req.path.strip_prefix("/play").unwrap_or("");
    let rel = match rel {
        "" => {
            respond(w, "301 Moved Permanently", "text/plain", "Location: /play/\r\n", b"moved\n", head_only, false);
            return Next::Close;
        }
        "/" => "/index.html",
        r => r,
    };
    let Some(f) = ctx.site.files.get(rel) else {
        let msg: &[u8] = if ctx.site.is_empty() { b"the browser version isn't installed on this server\n" } else { b"not found\n" };
        return if respond(w, "404 Not Found", "text/plain", "", msg, head_only, true) { Next::KeepAlive } else { Next::Close };
    };
    let keep = !req.has_token("connection", "close");
    let csp = "Content-Security-Policy: default-src 'self'; connect-src 'self'; script-src 'self' 'wasm-unsafe-eval'; \
style-src 'self'; img-src 'self' data: blob:; media-src 'self' blob:; frame-ancestors 'none'; base-uri 'none'; form-action 'none'\r\n\
Referrer-Policy: no-referrer\r\nX-Frame-Options: DENY\r\n";
    // index.html and the code change with every release; sprites rarely, and the ETag catches it
    let cache = if rel.starts_with("/assets/") { "Cache-Control: max-age=3600\r\n" } else { "Cache-Control: no-cache\r\n" };
    let extra = format!("{csp}{cache}ETag: {}\r\n", f.etag);
    if req.header("if-none-match") == Some(f.etag.as_str()) {
        let ok = w.write_all(format!("HTTP/1.1 304 Not Modified\r\nConnection: {}\r\n{extra}\r\n", if keep { "keep-alive" } else { "close" }).as_bytes()).is_ok() && w.flush().is_ok();
        return if ok && keep { Next::KeepAlive } else { Next::Close };
    }
    let Ok(mut file) = std::fs::File::open(&f.path) else {
        respond(w, "404 Not Found", "text/plain", "", b"not found\n", head_only, false);
        return Next::Close;
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {len}\r\nConnection: {}\r\nX-Content-Type-Options: nosniff\r\n{extra}\r\n",
        f.ctype,
        if keep { "keep-alive" } else { "close" }
    );
    if w.write_all(head.as_bytes()).is_err() {
        return Next::Close;
    }
    if !head_only {
        // exactly Content-Length bytes, or the keep-alive stream is out of step
        match std::io::copy(&mut (&mut file).take(len), w) {
            Ok(n) if n == len => {}
            _ => return Next::Close,
        }
    }
    if w.flush().is_ok() && keep {
        Next::KeepAlive
    } else {
        Next::Close
    }
}

fn websocket(req: &Request, w: &mut dyn Write, ctx: &Ctx) -> Next {
    if !req.is_websocket() {
        respond(w, "400 Bad Request", "text/plain", "", b"websocket handshake expected\n", false, false);
        return Next::Close;
    }
    if !origin_ok(req.header("origin"), ctx) {
        // not an auth failure (a typo'd host or the raw IP auto-reconnects); the Page bucket limits it
        respond(w, "403 Forbidden", "text/plain", "", b"origin not allowed\n", false, false);
        return Next::Close;
    }
    if !ctx.game_ok {
        // plaintext from outside the LAN: passwords would cross the internet in the clear
        respond(w, "403 Forbidden", "text/plain", "", b"use https\n", false, false);
        return Next::Close;
    }
    let key = req.header("sec-websocket-key").unwrap_or("");
    let head = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        crate::ws::accept_key(key)
    );
    if w.write_all(head.as_bytes()).is_ok() && w.flush().is_ok() {
        Next::WebSocket
    } else {
        Next::Close
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;
    use std::net::{TcpListener, TcpStream};

    fn get(port: u16, req: &str) -> String {
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.write_all(req.as_bytes()).unwrap();
        let mut out = String::new();
        let _ = c.read_to_string(&mut out);
        out
    }

    fn server(dir: PathBuf, origins: Vec<String>) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let site = Site::scan(&dir.join("public").join("web"));
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let ip = s.peer_addr().unwrap().ip();
                let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
                let mut w = s.try_clone().unwrap();
                let mut r = BufReader::new(s);
                let ctx = Ctx { ip, data: &dir, site: &site, secure: false, game_ok: true, origins: &origins, https_base: None, head_timeout: Duration::from_secs(2) };
                if let Outcome::WebSocket = serve(&mut r, &mut w, &ctx) {
                    let _ = w.write_all(b"UPGRADED");
                }
            }
        });
        port
    }

    #[test]
    fn serves_page_apk_play_and_404() {
        let dir = std::env::temp_dir().join(format!("httptest-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("public/web/assets")).unwrap();
        std::fs::write(dir.join("public").join(APK_NAME), b"APKDATA").unwrap();
        std::fs::write(dir.join("public").join(format!("{APK_NAME}.sha256")), format!("{}  {APK_NAME}\n", "ab".repeat(32))).unwrap();
        std::fs::write(dir.join("public/web/index.html"), b"<html>game</html>").unwrap();
        std::fs::write(dir.join("public/web/hohmann.wasm"), b"\0asm").unwrap();
        std::fs::write(dir.join("public/web/assets/a.png"), b"PNG").unwrap();
        std::fs::write(dir.join("public/web/secret.txt"), b"no").unwrap();
        std::fs::write(dir.join("accounts.json"), b"{}").unwrap();
        let port = server(dir.clone(), vec![]);
        let page = get(port, "GET / HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(page.starts_with("HTTP/1.1 200") && page.contains(&"ab".repeat(32)) && page.contains("/play/"), "{page}");
        let apk = get(port, &format!("GET /{APK_NAME} HTTP/1.1\r\n\r\n"));
        assert!(apk.ends_with("APKDATA") && apk.contains("android.package-archive"), "{apk}");
        assert!(get(port, "GET /../accounts.json HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 400"));
        assert!(get(port, "GET /play/%2e%2e/accounts.json HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 400"));
        assert!(get(port, "GET /play/secret.txt HTTP/1.1\r\nConnection: close\r\n\r\n").starts_with("HTTP/1.1 404"));
        assert!(get(port, "GET /accounts.json HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 404"));
        assert!(get(port, "POST / HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 405"));
        assert!(get(port, "GET / HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello").is_empty(), "bodies refused");
        assert!(get(port, &format!("GET / HTTP/1.1\r\n{}\r\n\r\n", "X: y\r\n".repeat(2000))).is_empty(), "head too big");
        assert!(get(port, &format!("GET / HTTP/1.1\r\n{}\r\n\r\n", "X: y\r\n".repeat(51))).is_empty(), "too many headers");
        assert!(get(port, "GET /play HTTP/1.1\r\n\r\n").contains("Location: /play/"));
        // keep-alive: three requests on one connection, the wasm with its MIME type and a strict CSP
        let both = get(port, "GET /play/ HTTP/1.1\r\nHost: h:7777\r\n\r\nGET /play/hohmann.wasm HTTP/1.1\r\n\r\nGET /play/assets/a.png HTTP/1.1\r\nConnection: close\r\n\r\n");
        assert_eq!(both.matches("HTTP/1.1 200").count(), 3, "{both}");
        assert!(both.contains("<html>game</html>") && both.contains("application/wasm") && both.ends_with("PNG"));
        assert!(both.contains("script-src 'self' 'wasm-unsafe-eval'") && both.contains("connect-src 'self';"));
        let etag = both.split("ETag: ").nth(1).unwrap().split("\r\n").next().unwrap().to_string();
        assert!(get(port, &format!("GET /play/ HTTP/1.1\r\nIf-None-Match: {etag}\r\nConnection: close\r\n\r\n")).starts_with("HTTP/1.1 304"));
        assert!(looks_like_http(b"GET ") && looks_like_http(b"HEAD") && !looks_like_http(b"{\"He"));
    }

    #[test]
    fn websocket_handshake_and_origins() {
        let dir = std::env::temp_dir().join(format!("httptest-ws-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let port = server(dir, vec!["https://game.example:7777".into()]);
        let hs = |extra: &str| {
            get(port, &format!("GET /ws HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: keep-alive, Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n{extra}\r\n"))
        };
        let ok = hs("");
        assert!(ok.starts_with("HTTP/1.1 101") && ok.contains("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=") && ok.ends_with("UPGRADED"), "{ok}");
        assert!(hs("Origin: https://game.example:7777\r\n").starts_with("HTTP/1.1 101"));
        assert!(hs("Origin: http://192.168.0.105:7777\r\n").starts_with("HTTP/1.1 101"), "LAN page over plain http");
        assert!(hs("Origin: https://evil.example\r\n").starts_with("HTTP/1.1 403"));
        assert!(hs("Origin: http://evil.example:7777\r\n").starts_with("HTTP/1.1 403"));
        assert!(get(port, "GET /ws HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 400"));
    }
}

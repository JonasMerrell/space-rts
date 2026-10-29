use std::net::TcpListener;
use std::path::PathBuf;

const USAGE: &str = "usage: server [--port 7777] [--data DIR] [--tls-cert PEM --tls-key PEM] [--no-plain] [--invite-codes FILE]
              [--web-cert PEM --web-key PEM] [--web-host NAME]
  --tls-cert/--tls-key  certificate chain + key (e.g. Let's Encrypt fullchain.pem/privkey.pem);
                        default: a self-signed cert made once in DIR/tls/
  --no-plain            refuse unencrypted connections even from the LAN
  --invite-codes FILE   registering needs a code from FILE (one per line, each works once)
  --make-invites N      append N new random codes to the --invite-codes FILE, print them and exit
  --web-cert/--web-key  CA-signed cert + key for browsers (Let's Encrypt fullchain.pem/privkey.pem),
                        used when a browser connects (ALPN); the app keeps the pinned cert.
                        Re-read when the file changes. The browser build is served at /play/
                        from DIR/public/web/ (scripts/build-web.sh)
  --web-host NAME       public name of the browser page (allowed WebSocket origin; repeatable)
  --latest-client VER   newest released app version, told to apps at login so older ones ask to update;
                        default: the version in DIR/public/web/version.json (from the web release)
  --update-url URL      page the app's UPDATE button opens, e.g. this server's download page
                        https://hohmannsreach.duckdns.org:7777/ (default: that page, built into the app)";

fn main() {
    let mut port = proto::DEFAULT_PORT;
    let mut cfg = server::Config::new(PathBuf::from("data"));
    let mut make = None;
    let mut args = std::env::args().skip(1);
    let usage = || -> ! {
        eprintln!("{USAGE}");
        std::process::exit(2)
    };
    while let Some(a) = args.next() {
        let mut path = || args.next().map(PathBuf::from).unwrap_or_else(|| usage());
        match a.as_str() {
            "--port" => port = path().to_str().and_then(|p| p.parse().ok()).unwrap_or_else(|| usage()),
            "--data" => cfg.data = path(),
            "--tls-cert" => cfg.tls_cert = Some(path()),
            "--tls-key" => cfg.tls_key = Some(path()),
            "--invite-codes" => cfg.invites = Some(path()),
            "--no-plain" => cfg.plain = server::Plain::Never,
            "--web-cert" => cfg.web_cert = Some(path()),
            "--web-key" => cfg.web_key = Some(path()),
            "--latest-client" => cfg.latest_client = Some(path().to_string_lossy().to_string()),
            "--update-url" => {
                let u = path().to_string_lossy().to_string();
                if !proto::ok_update_url(&u) {
                    eprintln!("--update-url must be an http(s) URL");
                    usage();
                }
                cfg.update_url = Some(u);
            }
            "--web-host" => cfg.web_hosts.push(path().to_string_lossy().to_string()),
            "--make-invites" => make = Some(path().to_str().and_then(|n| n.parse::<usize>().ok()).unwrap_or_else(|| usage())),
            _ => usage(),
        }
    }
    if let Some(n) = make {
        let Some(file) = &cfg.invites else { usage() };
        match server::make_invites(file, n) {
            Ok(codes) => codes.iter().for_each(|c| println!("{c}")),
            Err(e) => {
                eprintln!("{}: {e}", file.display());
                std::process::exit(1);
            }
        }
        return;
    }
    if cfg.web_hosts.is_empty() {
        cfg.web_hosts.push("hohmannsreach.tplinkdns.com".into());
    }
    let listener = match TcpListener::bind(("0.0.0.0", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("can't listen on :{port}: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("Hohmann's Reach server on :{port}");
    if let Err(e) = server::serve_with(listener, cfg) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

use std::net::TcpListener;
use std::path::PathBuf;

const USAGE: &str = "usage: server [--port 7777] [--data DIR] [--tls-cert PEM --tls-key PEM] [--no-plain] [--invite-codes FILE]
  --tls-cert/--tls-key  certificate chain + key (e.g. Let's Encrypt fullchain.pem/privkey.pem);
                        default: a self-signed cert made once in DIR/tls/
  --no-plain            refuse unencrypted connections even from the LAN
  --invite-codes FILE   registering needs a code from FILE (one per line, each works once)";

fn main() {
    let mut port = proto::DEFAULT_PORT;
    let mut cfg = server::Config::new(PathBuf::from("data"));
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
            _ => usage(),
        }
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

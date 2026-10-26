use std::net::TcpListener;
use std::path::PathBuf;

fn main() {
    let mut port = proto::DEFAULT_PORT;
    let mut data = PathBuf::from("data");
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => port = args.next().and_then(|p| p.parse().ok()).expect("--port N"),
            "--data" => data = args.next().map(PathBuf::from).expect("--data DIR"),
            _ => {
                eprintln!("usage: server [--port {}] [--data DIR]", proto::DEFAULT_PORT);
                std::process::exit(2);
            }
        }
    }
    let listener = TcpListener::bind(("0.0.0.0", port)).expect("bind");
    eprintln!("space-rts server on :{port}");
    server::serve(listener, data);
}

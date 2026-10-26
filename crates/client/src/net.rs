//! Background TLS connection to the game server (JSON lines). Never blocks the render loop.
use crate::tls;
use proto::{decode, encode, wire, ClientMsg, ServerMsg};
use std::io::{BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::Duration;

pub enum NetEvent {
    Connected,
    Msg(ServerMsg),
    Failed(String),
    Closed,
}

pub struct Net {
    tx: Sender<ClientMsg>,
    rx: Receiver<NetEvent>,
}

impl Net {
    /// Connect in the background; `hello` is sent first once connected.
    pub fn connect(addr: String, hello: ClientMsg) -> Net {
        let (tx, out_rx) = channel::<ClientMsg>();
        let (in_tx, rx) = channel::<NetEvent>();
        thread::spawn(move || run(addr, hello, out_rx, in_tx));
        Net { tx, rx }
    }

    pub fn send(&self, m: ClientMsg) {
        let _ = self.tx.send(m);
    }

    pub fn poll(&self) -> Vec<NetEvent> {
        let mut out = vec![];
        loop {
            match self.rx.try_recv() {
                Ok(e) => out.push(e),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    out.push(NetEvent::Closed);
                    break;
                }
            }
        }
        out
    }
}

fn run(addr: String, hello: ClientMsg, out_rx: Receiver<ClientMsg>, in_tx: Sender<NetEvent>) {
    let (addr, pin) = tls::split_pin(&addr);
    let addr = if addr.contains(':') { addr } else { format!("{addr}:{}", proto::DEFAULT_PORT) };
    let fail = |m: String| {
        let _ = in_tx.send(NetEvent::Failed(m));
    };
    // the public name first; on home Wi-Fi the router may not loop it back, so fall back to the LAN address
    let mut candidates = vec![addr.clone()];
    if let Some(lan) = crate::lan_fallback(&addr) {
        candidates.push(lan.to_string());
    }
    let mut last_err = String::new();
    let mut found = None;
    for cand in &candidates {
        let Some(sock) = cand.to_socket_addrs().ok().and_then(|mut a| a.next()) else {
            last_err = format!("can't resolve {cand}");
            continue;
        };
        match TcpStream::connect_timeout(&sock, Duration::from_secs(5)) {
            Ok(s) => {
                found = Some((cand.clone(), s));
                break;
            }
            Err(e) => last_err = format!("can't reach {cand}: {e}"),
        }
    }
    let Some((addr, stream)) = found else { return fail(last_err) };
    let _ = stream.set_nodelay(true);
    let (cfg, first_seen) = match tls::config(&addr, pin) {
        Ok(c) => c,
        Err(e) => return fail(format!("TLS setup: {e}")),
    };
    let conn = match tls::server_name(&addr).and_then(|n| rustls::ClientConnection::new(cfg, n).map_err(|e| e.to_string())) {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let Ok(sock2) = stream.try_clone() else { return fail("socket error".into()) };
    let (r, mut w) = match wire::split(conn, sock2) {
        Ok(rw) => rw,
        Err(e) => {
            let msg = e.to_string();
            return fail(if msg.contains(tls::CHANGED) {
                format!("{addr}: {}. If you trust it, re-enter the server as {addr}#<new sha256>, or reinstall.", tls::CHANGED)
            } else {
                format!("secure connection to {addr} failed: {msg}")
            });
        }
    };
    tls::remember(&addr, &first_seen);
    // the app's heartbeat notices a dead server; no socket timeout once connected
    let _ = stream.set_read_timeout(None);
    let _ = in_tx.send(NetEvent::Connected);
    // writer
    thread::spawn(move || {
        if w.write_all(encode(&hello).as_bytes()).is_err() {
            return;
        }
        for m in out_rx {
            if w.write_all(encode(&m).as_bytes()).is_err() {
                break;
            }
        }
        w.shutdown();
    });
    // reader
    let mut reader = BufReader::with_capacity(1 << 20, r);
    while let Ok(Some(line)) = wire::read_line(&mut reader, wire::MAX_SERVER_LINE) {
        match decode::<ServerMsg>(&line) {
            Ok(m) => {
                if in_tx.send(NetEvent::Msg(m)).is_err() {
                    return;
                }
            }
            Err(e) => eprintln!("bad server message: {e}"),
        }
    }
    let _ = in_tx.send(NetEvent::Closed);
}

//! Background TCP connection to the game server (JSON lines). Never blocks the render loop.
use proto::{decode, encode, ClientMsg, ServerMsg};
use std::io::{BufRead, BufReader, Write};
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
    let addr = if addr.contains(':') { addr } else { format!("{addr}:{}", proto::DEFAULT_PORT) };
    let sock = match addr.to_socket_addrs().ok().and_then(|mut a| a.next()) {
        Some(a) => a,
        None => {
            let _ = in_tx.send(NetEvent::Failed(format!("can't resolve {addr}")));
            return;
        }
    };
    let stream = match TcpStream::connect_timeout(&sock, Duration::from_secs(6)) {
        Ok(s) => s,
        Err(e) => {
            let _ = in_tx.send(NetEvent::Failed(format!("can't reach {addr}: {e}")));
            return;
        }
    };
    let _ = stream.set_nodelay(true);
    let _ = in_tx.send(NetEvent::Connected);
    let mut w = stream.try_clone().expect("clone");
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
        let _ = w.shutdown(std::net::Shutdown::Both);
    });
    // reader
    let reader = BufReader::with_capacity(1 << 20, stream);
    for line in reader.lines() {
        let Ok(line) = line else { break };
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

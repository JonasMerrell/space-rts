//! WebSocket (RFC 6455) for the browser client, behind the same `BufRead`/`Write` pair the JSON-lines
//! loop uses: each client text message reads as one line, each line the server writes goes out as
//! one text frame. Only what browsers need: masked client frames, text, ping/pong, close.
use std::io::{self, BufRead, Read, Write};
use std::sync::{Arc, Mutex};

/// Largest client frame, and largest reassembled client message (same as a JSON line).
pub const MAX_MESSAGE: usize = proto::wire::MAX_CLIENT_LINE;
const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// `Sec-WebSocket-Accept` for a client's `Sec-WebSocket-Key`.
pub fn accept_key(key: &str) -> String {
    use sha1::{Digest, Sha1};
    base64(&Sha1::digest(format!("{}{GUID}", key.trim()).as_bytes()))
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The raw stream's write half, shared so the reader can answer pings and closes.
type Raw = Arc<Mutex<RawW>>;

struct RawW {
    w: Box<dyn Write + Send>,
    /// a close frame went out; nothing may follow it
    closed: bool,
}

/// Most control frames (ping/pong) in a row without a data frame before we hang up.
const MAX_CONTROL_RUN: u32 = 20;

fn frame(raw: &Raw, opcode: u8, payload: &[u8]) -> io::Result<()> {
    let mut head = vec![0x80 | opcode];
    match payload.len() {
        n if n < 126 => head.push(n as u8),
        n if n <= 0xffff => {
            head.push(126);
            head.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            head.push(127);
            head.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    let mut g = raw.lock().unwrap_or_else(|e| e.into_inner());
    if g.closed {
        return Err(io::Error::new(io::ErrorKind::BrokenPipe, "websocket closed"));
    }
    g.closed = opcode == 0x8;
    g.w.write_all(&head)?;
    g.w.write_all(payload)?;
    g.w.flush()
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

/// Split an upgraded connection into a line reader and a line writer.
pub fn split(r: Box<dyn BufRead + Send>, w: Box<dyn Write + Send>) -> (WsReader, WsWriter) {
    let raw: Raw = Arc::new(Mutex::new(RawW { w, closed: false }));
    (WsReader { inner: r, raw: raw.clone(), msg: vec![], frag: false, out: vec![], pos: 0, closed: false, controls: 0 }, WsWriter { raw, line: vec![] })
}

pub struct WsReader {
    inner: Box<dyn BufRead + Send>,
    raw: Raw,
    /// text message being reassembled from fragments
    msg: Vec<u8>,
    /// inside a fragmented message
    frag: bool,
    /// complete message + '\n', handed out through `BufRead`
    out: Vec<u8>,
    pos: usize,
    closed: bool,
    /// control frames since the last data frame
    controls: u32,
}

impl WsReader {
    /// Read frames until a whole text message is in `out`. Ok(false) = the client closed.
    fn next_message(&mut self) -> io::Result<bool> {
        loop {
            if self.closed {
                return Ok(false);
            }
            let mut h = [0u8; 2];
            if let Err(e) = self.inner.read_exact(&mut h) {
                return if e.kind() == io::ErrorKind::UnexpectedEof { Ok(false) } else { Err(e) };
            }
            let (fin, opcode, masked) = (h[0] & 0x80 != 0, h[0] & 0x0f, h[1] & 0x80 != 0);
            if h[0] & 0x70 != 0 {
                return Err(bad("websocket extension bits set"));
            }
            if !masked {
                return Err(bad("unmasked client frame"));
            }
            let len = match h[1] & 0x7f {
                126 => {
                    let mut b = [0u8; 2];
                    self.inner.read_exact(&mut b)?;
                    u16::from_be_bytes(b) as u64
                }
                127 => {
                    let mut b = [0u8; 8];
                    self.inner.read_exact(&mut b)?;
                    u64::from_be_bytes(b)
                }
                n => n as u64,
            };
            let control = opcode & 0x8 != 0;
            if control && (len > 125 || !fin) {
                return Err(bad("bad control frame"));
            }
            // pings skip the line loop's message bucket; don't let them flood either
            self.controls = if control { self.controls + 1 } else { 0 };
            if self.controls > MAX_CONTROL_RUN {
                return Err(bad("too many control frames"));
            }
            // checked before allocating anything
            if len as usize > MAX_MESSAGE || self.msg.len() + len as usize > MAX_MESSAGE {
                return Err(bad("line too long"));
            }
            let mut mask = [0u8; 4];
            self.inner.read_exact(&mut mask)?;
            let mut payload = vec![0u8; len as usize];
            self.inner.read_exact(&mut payload)?;
            for (i, b) in payload.iter_mut().enumerate() {
                *b ^= mask[i % 4];
            }
            match opcode {
                0x1 if !self.frag => self.msg = payload,
                0x0 if self.frag => self.msg.extend_from_slice(&payload),
                0x8 => {
                    // echo the status code back and stop reading
                    let _ = frame(&self.raw, 0x8, &payload[..payload.len().min(2)]);
                    self.closed = true;
                    return Ok(false);
                }
                0x9 => {
                    frame(&self.raw, 0xA, &payload)?;
                    continue;
                }
                0xA => continue,
                0x2 => return Err(bad("binary websocket messages aren't used")),
                _ => return Err(bad("unexpected websocket frame")),
            }
            self.frag = !fin;
            if fin {
                self.out = std::mem::take(&mut self.msg);
                self.out.push(b'\n');
                self.pos = 0;
                return Ok(true);
            }
        }
    }
}

impl Read for WsReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let avail = self.fill_buf()?;
        let n = avail.len().min(buf.len());
        buf[..n].copy_from_slice(&avail[..n]);
        self.consume(n);
        Ok(n)
    }
}

impl BufRead for WsReader {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.pos >= self.out.len() && !self.next_message()? {
            return Ok(&[]);
        }
        Ok(&self.out[self.pos..])
    }
    fn consume(&mut self, n: usize) {
        self.pos += n;
    }
}

pub struct WsWriter {
    raw: Raw,
    line: Vec<u8>,
}

impl Write for WsWriter {
    /// Buffers until '\n'; every line goes out as one text frame (without the newline).
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let mut rest = data;
        while let Some(i) = rest.iter().position(|&b| b == b'\n') {
            self.line.extend_from_slice(&rest[..i]);
            frame(&self.raw, 0x1, &self.line)?;
            self.line.clear();
            rest = &rest[i + 1..];
        }
        self.line.extend_from_slice(rest);
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for WsWriter {
    fn drop(&mut self) {
        let _ = frame(&self.raw, 0x8, &1000u16.to_be_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A masked client frame.
    pub fn client_frame(fin: bool, opcode: u8, payload: &[u8]) -> Vec<u8> {
        let mask = [0x37, 0xfa, 0x21, 0x3d];
        let mut f = vec![(fin as u8) << 7 | opcode];
        match payload.len() {
            n if n < 126 => f.push(0x80 | n as u8),
            n if n <= 0xffff => {
                f.push(0x80 | 126);
                f.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                f.push(0x80 | 127);
                f.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        f.extend_from_slice(&mask);
        f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        f
    }

    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl Write for Sink {
        fn write(&mut self, d: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(d);
            Ok(d.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn reader(input: Vec<u8>) -> (WsReader, WsWriter, Sink) {
        let sink = Sink::default();
        let (r, w) = split(Box::new(io::Cursor::new(input)), Box::new(sink.clone()));
        (r, w, sink)
    }

    #[test]
    fn accept_key_matches_rfc() {
        assert_eq!(accept_key("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"a"), "YQ==");
    }

    #[test]
    fn messages_become_lines() {
        let mut input = client_frame(true, 1, b"\"Ping\"");
        input.extend(client_frame(true, 9, b"hi")); // ping between messages
        input.extend(client_frame(false, 1, b"{\"a\":"));
        input.extend(client_frame(true, 0, b"1}"));
        input.extend(client_frame(true, 8, &1000u16.to_be_bytes()));
        input.extend(client_frame(true, 1, b"after close"));
        let (mut r, _w, sink) = reader(input);
        let max = proto::wire::MAX_CLIENT_LINE;
        assert_eq!(proto::wire::read_line(&mut r, max).unwrap().as_deref(), Some("\"Ping\""));
        assert_eq!(proto::wire::read_line(&mut r, max).unwrap().as_deref(), Some("{\"a\":1}"));
        assert_eq!(proto::wire::read_line(&mut r, max).unwrap(), None);
        let out = sink.0.lock().unwrap().clone();
        assert_eq!(&out[..4], &[0x8A, 2, b'h', b'i'], "pong");
        assert_eq!(&out[4..], &[0x88, 2, 0x03, 0xE8], "close echoed");
        drop(_w);
        assert_eq!(sink.0.lock().unwrap().len(), out.len(), "no second close frame");
    }

    #[test]
    fn rejects_hostile_frames() {
        let max = proto::wire::MAX_CLIENT_LINE;
        let err = |input: Vec<u8>| proto::wire::read_line(&mut reader(input).0, max).is_err();
        // unmasked
        assert!(err(vec![0x81, 0x02, b'h', b'i']));
        // binary
        assert!(err(client_frame(true, 2, b"xx")));
        // huge length claimed: refused before reading/allocating it
        assert!(err(vec![0x81, 0x80 | 127, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0]));
        assert!(err(client_frame(true, 1, &vec![b'a'; MAX_MESSAGE + 1])));
        // fragments adding up past the cap
        let mut f = client_frame(false, 1, &vec![b'a'; MAX_MESSAGE / 2 + 1]);
        f.extend(client_frame(true, 0, &vec![b'a'; MAX_MESSAGE / 2 + 1]));
        assert!(err(f));
        // a ping flood
        assert!(err((0..=MAX_CONTROL_RUN).flat_map(|_| client_frame(true, 9, b"p")).collect()));
        // fragmented ping, oversized ping, continuation without a start, reserved bits, unknown opcode
        assert!(err(client_frame(false, 9, b"x")));
        assert!(err(client_frame(true, 9, &[0; 126])));
        assert!(err(client_frame(true, 0, b"x")));
        assert!(err({
            let mut f = client_frame(true, 1, b"x");
            f[0] |= 0x40;
            f
        }));
        assert!(err(client_frame(true, 3, b"x")));
        // a cut-off frame is an error, a clean end is EOF
        assert!(err(client_frame(true, 1, b"hello")[..5].to_vec()));
        assert_eq!(proto::wire::read_line(&mut reader(vec![]).0, max).unwrap(), None);
    }

    #[test]
    fn lines_become_frames() {
        let (_r, mut w, sink) = reader(vec![]);
        w.write_all(b"{\"x\":1}\n{\"y\"").unwrap();
        w.write_all(b":2}\n").unwrap();
        let big = vec![b'z'; 70_000];
        w.write_all(&big).unwrap();
        w.write_all(b"\n").unwrap();
        let out = sink.0.lock().unwrap().clone();
        assert_eq!(&out[..9], b"\x81\x07{\"x\":1}");
        assert_eq!(&out[9..18], b"\x81\x07{\"y\":2}");
        assert_eq!(&out[18..28], &[0x81, 127, 0, 0, 0, 0, 0, 1, 0x11, 0x70]);
        assert_eq!(out.len(), 28 + 70_000);
    }
}

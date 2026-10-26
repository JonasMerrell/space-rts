//! Transport shared by client and server: bounded line reads, and a TLS connection split into a
//! reader and a writer half so each can live on its own thread (like `TcpStream::try_clone`).
use std::io::{self, BufRead, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};

/// Most a client may send in one line. Commands are tiny; this leaves lots of headroom.
pub const MAX_CLIENT_LINE: usize = 64 * 1024;
/// Most the server sends in one line (a full `State` of a big system can be a few MB).
pub const MAX_SERVER_LINE: usize = 64 * 1024 * 1024;

/// Read one `\n`-terminated line of at most `max` bytes (without the newline). `Ok(None)` at EOF.
/// A longer line or invalid UTF-8 is an error, and the caller should drop the connection.
pub fn read_line<R: BufRead>(r: &mut R, max: usize) -> io::Result<Option<String>> {
    let mut buf = Vec::new();
    loop {
        let avail = match r.fill_buf() {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if avail.is_empty() {
            return if buf.is_empty() { Ok(None) } else { Err(io::Error::new(io::ErrorKind::UnexpectedEof, "line cut off")) };
        }
        let (n, done) = match avail.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (avail.len(), false),
        };
        if buf.len() + n > max + 1 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "line too long"));
        }
        buf.extend_from_slice(&avail[..n]);
        r.consume(n);
        if done {
            buf.pop();
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
            return String::from_utf8(buf).map(Some).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not UTF-8"));
        }
    }
}

/// A TLS session over a TCP socket, shared by a `TlsReader` and a `TlsWriter`.
type Shared = Arc<Mutex<rustls::Connection>>;

fn lock(c: &Shared) -> std::sync::MutexGuard<'_, rustls::Connection> {
    c.lock().unwrap_or_else(|e| e.into_inner())
}

fn tls_err(e: rustls::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

/// Run the handshake to completion on `sock` (which should have read/write timeouts set), then
/// split. Call `.0` for reads and `.1` for writes from different threads.
pub fn split<C: Into<rustls::Connection>>(conn: C, sock: TcpStream) -> io::Result<(TlsReader, TlsWriter)> {
    let mut conn: rustls::Connection = conn.into();
    let mut s = &sock;
    while conn.is_handshaking() {
        conn.complete_io(&mut s)?;
    }
    // flush whatever the handshake left (e.g. TLS 1.3 session tickets)
    while conn.wants_write() {
        conn.write_tls(&mut s)?;
    }
    let shared = Arc::new(Mutex::new(conn));
    let w = TlsWriter { conn: shared.clone(), sock: sock.try_clone()? };
    Ok((TlsReader { conn: shared, sock, raw: vec![0; 16 * 1024] }, w))
}

pub struct TlsReader {
    conn: Shared,
    sock: TcpStream,
    raw: Vec<u8>,
}

impl TlsReader {
    /// Send whatever the session wants to send (alerts, key updates) after processing input.
    fn flush(&self, c: &mut rustls::Connection) -> io::Result<()> {
        let mut s = &self.sock;
        while c.wants_write() {
            c.write_tls(&mut s)?;
        }
        Ok(())
    }
}

impl Read for TlsReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        loop {
            {
                let mut c = lock(&self.conn);
                match c.reader().read(out) {
                    Ok(n) => return Ok(n), // 0 = peer sent close_notify
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e),
                }
            }
            // block on the socket without holding the lock, so the writer keeps going
            let n = (&self.sock).read(&mut self.raw)?;
            if n == 0 {
                return Ok(0);
            }
            let mut c = lock(&self.conn);
            let mut fed = &self.raw[..n];
            while !fed.is_empty() {
                c.read_tls(&mut fed)?;
                if let Err(e) = c.process_new_packets() {
                    let _ = self.flush(&mut c); // send the alert
                    return Err(tls_err(e));
                }
            }
            self.flush(&mut c)?;
        }
    }
}

pub struct TlsWriter {
    conn: Shared,
    sock: TcpStream,
}

impl TlsWriter {
    pub fn shutdown(&self) {
        let mut c = lock(&self.conn);
        c.send_close_notify();
        let mut s = &self.sock;
        let _ = c.write_tls(&mut s);
        let _ = self.sock.shutdown(std::net::Shutdown::Both);
    }
}

impl Write for TlsWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let mut c = lock(&self.conn);
        let n = c.writer().write(data)?;
        let mut s = &self.sock;
        while c.wants_write() {
            c.write_tls(&mut s)?;
        }
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(input: &[u8], max: usize) -> Vec<Result<Option<String>, io::ErrorKind>> {
        let mut r = io::BufReader::with_capacity(4, input);
        let mut out = vec![];
        loop {
            let x = read_line(&mut r, max).map_err(|e| e.kind());
            let stop = !matches!(x, Ok(Some(_)));
            out.push(x);
            if stop {
                return out;
            }
        }
    }

    #[test]
    fn bounded_lines() {
        assert_eq!(lines(b"ab\r\ncd\n", 8), vec![Ok(Some("ab".into())), Ok(Some("cd".into())), Ok(None)]);
        assert_eq!(lines(b"12345678\n", 8), vec![Ok(Some("12345678".into())), Ok(None)]);
        assert_eq!(lines(b"123456789\n", 8), vec![Err(io::ErrorKind::InvalidData)]);
        assert_eq!(lines(b"no newline", 100), vec![Err(io::ErrorKind::UnexpectedEof)]);
        assert_eq!(lines(b"\xff\xfe\n", 100), vec![Err(io::ErrorKind::InvalidData)]);
    }
}

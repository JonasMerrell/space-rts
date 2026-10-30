//! Push notifications for the events that can't wait for the apps' 15-minute poll: a battle starting
//! and enemies on their way (live battles, Jonas 2026-10-29).
//!
//! Browsers: Web Push (RFC 8030) with the payload encrypted for the subscription (RFC 8291,
//! aes128gcm) and the server identified by a VAPID key (RFC 8292) that is made on first start and
//! kept in `<data>/push-vapid.p8`. The page gets the public half from `GET /play/push-key`.
//!
//! Sending happens on a worker thread so a slow push service never holds up the game. Subscriptions
//! the push service says are gone (404/410) are handed back to be forgotten.
use proto::PushSub;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use ring::{aead, agreement, hkdf};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Event titles worth waking a phone for (the rest waits for the poll).
pub const TITLES: [&str; 3] = ["Enemies inbound", "Battle", "Under attack"];
/// Push services we send to. Anything else is refused, so an account can't point the server at
/// other hosts.
const HOSTS: [&str; 4] = [".googleapis.com", ".mozilla.com", ".windows.com", ".push.apple.com"];
/// Subscriptions kept per account (the oldest goes first).
pub const MAX_SUBS: usize = 8;
/// Pushes waiting to be sent; more are dropped.
const QUEUE: usize = 256;
/// How long the push service keeps a message for a device that's offline (s).
const TTL: u32 = 3600;
/// Who runs this server, for the push services (VAPID `sub`).
const CONTACT: &str = "https://github.com/JonasMerrell/space-rts";

/// What a notification shows: the event's title and text, and a tag that stops the same event
/// showing twice (the browser tab's own poll uses the same one).
#[derive(serde::Serialize, Clone, Debug)]
pub struct Note {
    pub title: String,
    pub body: String,
    pub tag: String,
}

pub struct Pusher {
    key: Arc<EcdsaKeyPair>,
    tx: SyncSender<(PushSub, Note)>,
    /// subscriptions the push service has dropped, to remove from the accounts
    pub gone: Arc<Mutex<Vec<String>>>,
}

impl Pusher {
    /// Load or make the VAPID key in `data` and start the sender thread.
    pub fn start(data: &Path) -> Result<Pusher, String> {
        let key = Arc::new(load_key(&data.join("push-vapid.p8"))?);
        let (tx, rx) = sync_channel::<(PushSub, Note)>(QUEUE);
        let gone = Arc::new(Mutex::new(vec![]));
        let (k, g) = (key.clone(), gone.clone());
        std::thread::spawn(move || {
            let tls = client_tls();
            for (sub, note) in rx {
                match send(&k, &tls, &sub, &note) {
                    Ok(404 | 410) => g.lock().unwrap_or_else(|e| e.into_inner()).push(sub.id().to_string()),
                    Ok(code) if !(200..300).contains(&code) => eprintln!("push to {}: HTTP {code}", host_of(&sub)),
                    Ok(_) => {}
                    Err(e) => eprintln!("push to {}: {e}", host_of(&sub)),
                }
            }
        });
        Ok(Pusher { key, tx, gone })
    }

    /// A pusher that hands its notifications to the returned receiver instead of sending them.
    #[cfg(test)]
    pub fn capture() -> (Pusher, std::sync::mpsc::Receiver<(PushSub, Note)>) {
        let rng = SystemRandom::new();
        let doc = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let key = Arc::new(EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, doc.as_ref(), &rng).unwrap());
        let (tx, rx) = sync_channel(QUEUE);
        (Pusher { key, tx, gone: Arc::default() }, rx)
    }

    /// The VAPID public key, base64url, for the browser's `PushManager.subscribe`.
    pub fn public_key(&self) -> String {
        b64(self.key.public_key().as_ref())
    }

    /// Queue a notification (dropped when the queue is full).
    pub fn push(&self, sub: &PushSub, note: Note) {
        if let Err(TrySendError::Full(_)) = self.tx.try_send((sub.clone(), note)) {
            eprintln!("push queue full, dropping");
        }
    }
}

/// Subscriptions we can deliver to: Web Push to a known push service, over https on 443.
pub fn acceptable(sub: &PushSub) -> bool {
    match sub {
        PushSub::Web { endpoint, p256dh, auth } => {
            let Some((host, port, _)) = split_url(endpoint) else { return false };
            port == 443 && HOSTS.iter().any(|h| host.ends_with(h)) && unb64(p256dh).is_some_and(|k| k.len() == 65) && unb64(auth).is_some_and(|a| a.len() == 16)
        }
        // kept for when the Android app gets Firebase; nothing is sent to these yet
        PushSub::Fcm { .. } => true,
    }
}

fn host_of(sub: &PushSub) -> String {
    match sub {
        PushSub::Web { endpoint, .. } => split_url(endpoint).map_or("?".into(), |(h, _, _)| h),
        PushSub::Fcm { .. } => "fcm".into(),
    }
}

fn load_key(path: &Path) -> Result<EcdsaKeyPair, String> {
    let rng = SystemRandom::new();
    let pk8 = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => {
            let doc = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).map_err(|_| "can't make a VAPID key")?;
            write_private(path, doc.as_ref()).map_err(|e| format!("can't save {}: {e}", path.display()))?;
            eprintln!("made a new push (VAPID) key in {}", path.display());
            doc.as_ref().to_vec()
        }
    };
    EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &pk8, &rng).map_err(|_| format!("bad VAPID key in {}", path.display()))
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    o.open(path)?.write_all(bytes)
}

fn client_tls() -> Arc<rustls::ClientConfig> {
    let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ClientConfig::builder_with_provider(provider).with_safe_default_protocol_versions().expect("tls versions").with_root_certificates(roots).with_no_client_auth();
    Arc::new(cfg)
}

/// POST one push; the HTTP status.
fn send(key: &EcdsaKeyPair, tls: &Arc<rustls::ClientConfig>, sub: &PushSub, note: &Note) -> Result<u16, String> {
    let PushSub::Web { endpoint, p256dh, auth } = sub else { return Ok(200) };
    let (host, port, path) = split_url(endpoint).ok_or("bad endpoint")?;
    let ua = unb64(p256dh).ok_or("bad p256dh")?;
    let secret = unb64(auth).ok_or("bad auth")?;
    let payload = serde_json::to_vec(note).map_err(|e| e.to_string())?;
    let body = encrypt(&payload, &ua, &secret)?;
    let jwt = vapid_jwt(key, &format!("https://{host}"), proto::unix_now() as u64 + 12 * 3600)?;
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nTTL: {TTL}\r\nUrgency: high\r\nContent-Encoding: aes128gcm\r\nContent-Type: application/octet-stream\r\n\
Content-Length: {}\r\nAuthorization: vapid t={jwt}, k={}\r\nConnection: close\r\n\r\n",
        body.len(),
        b64(key.public_key().as_ref())
    );
    let addr = (host.as_str(), port).to_socket_addrs().map_err(|e| e.to_string())?.next().ok_or("no address")?;
    let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(10)).map_err(|e| e.to_string())?;
    tcp.set_read_timeout(Some(Duration::from_secs(15))).ok();
    tcp.set_write_timeout(Some(Duration::from_secs(15))).ok();
    let name = rustls::pki_types::ServerName::try_from(host.clone()).map_err(|e| e.to_string())?;
    let conn = rustls::ClientConnection::new(tls.clone(), name).map_err(|e| e.to_string())?;
    let mut s = rustls::StreamOwned::new(conn, tcp);
    s.write_all(head.as_bytes()).and_then(|_| s.write_all(&body)).and_then(|_| s.flush()).map_err(|e| e.to_string())?;
    // only the status line matters
    let mut buf = [0u8; 64];
    let mut n = 0;
    while n < 12 {
        match s.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) => return Err(e.to_string()),
        }
    }
    let line = String::from_utf8_lossy(&buf[..n]);
    line.split_whitespace().nth(1).and_then(|c| c.parse().ok()).ok_or_else(|| "no HTTP status".to_string())
}

/// ("host", port, "/path?query") of an https URL.
fn split_url(url: &str) -> Option<(String, u16, String)> {
    let rest = url.strip_prefix("https://")?;
    let (auth, path) = rest.find('/').map_or((rest, "/"), |i| (&rest[..i], &rest[i..]));
    if auth.contains('@') || path.contains(['\r', '\n', ' ']) {
        return None;
    }
    let (host, port) = match auth.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().ok()?),
        None => (auth, 443),
    };
    let ok = !host.is_empty() && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    ok.then(|| (host.to_ascii_lowercase(), port, path.to_string()))
}

/// The VAPID token: an ES256 JWT for the push service's origin.
fn vapid_jwt(key: &EcdsaKeyPair, aud: &str, exp: u64) -> Result<String, String> {
    let head = b64(br#"{"typ":"JWT","alg":"ES256"}"#);
    let claims = b64(serde_json::json!({ "aud": aud, "exp": exp, "sub": CONTACT }).to_string().as_bytes());
    let msg = format!("{head}.{claims}");
    let sig = key.sign(&SystemRandom::new(), msg.as_bytes()).map_err(|_| "can't sign")?;
    Ok(format!("{msg}.{}", b64(sig.as_ref())))
}

/// RFC 8291 + RFC 8188: one aes128gcm record for the subscription's key `ua` (65 bytes) and auth
/// secret (16 bytes), from a fresh key pair and salt.
pub fn encrypt(plain: &[u8], ua: &[u8], secret: &[u8]) -> Result<Vec<u8>, String> {
    let rng = SystemRandom::new();
    let mut salt = [0u8; 16];
    rng.fill(&mut salt).map_err(|_| "rng")?;
    let me = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).map_err(|_| "keygen")?;
    let my_pub = me.compute_public_key().map_err(|_| "keygen")?;
    let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, ua);
    let shared = agreement::agree_ephemeral(me, &peer, |s| s.to_vec()).map_err(|_| "bad subscription key")?;
    encrypt_with(plain, ua, secret, my_pub.as_ref(), &shared, &salt)
}

fn encrypt_with(plain: &[u8], ua: &[u8], secret: &[u8], as_pub: &[u8], shared: &[u8], salt: &[u8; 16]) -> Result<Vec<u8>, String> {
    let (cek, nonce) = keys(ua, secret, as_pub, shared, salt);
    let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &cek).map_err(|_| "aes")?);
    let mut rec = plain.to_vec();
    rec.push(2); // last (only) record
    key.seal_in_place_append_tag(aead::Nonce::assume_unique_for_key(nonce), aead::Aad::empty(), &mut rec).map_err(|_| "seal")?;
    let mut out = Vec::with_capacity(86 + rec.len());
    out.extend_from_slice(salt);
    out.extend_from_slice(&4096u32.to_be_bytes());
    out.push(as_pub.len() as u8);
    out.extend_from_slice(as_pub);
    out.extend_from_slice(&rec);
    Ok(out)
}

/// (content encryption key, nonce) for a message.
fn keys(ua: &[u8], secret: &[u8], as_pub: &[u8], shared: &[u8], salt: &[u8]) -> ([u8; 16], [u8; 12]) {
    let info = [b"WebPush: info\0".as_slice(), ua, as_pub].concat();
    let ikm: [u8; 32] = hkdf_(secret, shared, &info);
    let cek: [u8; 16] = hkdf_(salt, &ikm, b"Content-Encoding: aes128gcm\0");
    let nonce: [u8; 12] = hkdf_(salt, &ikm, b"Content-Encoding: nonce\0");
    (cek, nonce)
}

struct Len(usize);
impl hkdf::KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

fn hkdf_<const N: usize>(salt: &[u8], ikm: &[u8], info: &[u8]) -> [u8; N] {
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, salt).extract(ikm);
    let mut out = [0u8; N];
    let infos = [info];
    prk.expand(&infos, Len(N)).and_then(|okm| okm.fill(&mut out)).expect("hkdf length");
    out
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// base64url without padding
pub fn b64(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = c.iter().enumerate().fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..=c.len() {
            s.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    s
}

/// base64url or standard base64, padding optional
pub fn unb64(s: &str) -> Option<Vec<u8>> {
    let s = s.trim_end_matches('=');
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut n, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        };
        n = n << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((n >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_round_trips() {
        for n in 0..40 {
            let v: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(unb64(&b64(&v)).unwrap(), v);
        }
        assert_eq!(b64(b"\xfb\xff"), "-_8");
        assert_eq!(unb64("+/8=").unwrap(), b"\xfb\xff");
    }

    /// RFC 8291 appendix A.
    #[test]
    fn matches_the_rfc_example() {
        let d = |s: &str| unb64(s).unwrap();
        let plain = b"When I grow up, I want to be a watermelon";
        let ua = d("BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4");
        let as_pub = d("BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8");
        let shared = d("kyrL1jIIOHEzg3sM2ZWRHDRB62YACZhhSlknJ672kSs");
        let secret = d("BTBZMqHH6r4Tts7J_aSIgg");
        let salt: [u8; 16] = d("DGv6ra1nlYgDCS1FRnbzlw").try_into().unwrap();
        let out = encrypt_with(plain, &ua, &secret, &as_pub, &shared, &salt).unwrap();
        let want = "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN";
        assert_eq!(b64(&out), want);
    }

    #[test]
    fn only_push_services_are_accepted() {
        let k = b64(&[4u8; 65]);
        let a = b64(&[1u8; 16]);
        let web = |e: &str| PushSub::Web { endpoint: e.into(), p256dh: k.clone(), auth: a.clone() };
        assert!(acceptable(&web("https://fcm.googleapis.com/fcm/send/abc:def")));
        assert!(acceptable(&web("https://updates.push.services.mozilla.com/wpush/v2/gAAA")));
        assert!(acceptable(&web("https://web.push.apple.com/QGz")));
        assert!(!acceptable(&web("https://192.168.0.105/x")));
        assert!(!acceptable(&web("https://evil.example.com/fcm.googleapis.com")));
        assert!(!acceptable(&web("https://fcm.googleapis.com:7777/x")));
        assert!(!acceptable(&web("https://user@fcm.googleapis.com/x")));
        assert!(!acceptable(&web("http://fcm.googleapis.com/x")));
        assert!(!acceptable(&PushSub::Web { endpoint: "https://fcm.googleapis.com/x".into(), p256dh: "AAAA".into(), auth: a.clone() }));
    }

    #[test]
    fn vapid_tokens_verify_with_the_public_key() {
        let dir = std::env::temp_dir().join(format!("vapid-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("k.p8");
        let _ = std::fs::remove_file(&p);
        let key = load_key(&p).unwrap();
        let again = load_key(&p).unwrap();
        assert_eq!(key.public_key().as_ref(), again.public_key().as_ref(), "the key is kept");
        let jwt = vapid_jwt(&key, "https://fcm.googleapis.com", 1_900_000_000).unwrap();
        let (msg, sig) = jwt.rsplit_once('.').unwrap();
        let pk = ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_FIXED, key.public_key().as_ref());
        pk.verify(msg.as_bytes(), &unb64(sig).unwrap()).unwrap();
        let claims: serde_json::Value = serde_json::from_slice(&unb64(msg.split('.').nth(1).unwrap()).unwrap()).unwrap();
        assert_eq!(claims["aud"], "https://fcm.googleapis.com");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

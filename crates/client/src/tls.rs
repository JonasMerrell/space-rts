//! Who the client trusts. A cert that chains to the public roots for the server's hostname is fine
//! (Let's Encrypt). Otherwise the server's own cert must match a pin: one baked into the build, one
//! written into the address as `host:port#<sha256 hex>`, or the one seen the first time we connected
//! to that address (trust on first use, remembered in `pins.txt`). A changed cert is refused.
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error, RootCertStore, SignatureScheme};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// A first-seen cert, remembered only once the handshake proved the server holds its key.
pub type FirstSeen = Arc<Mutex<Option<String>>>;

/// (address, cert sha256) known at build time, e.g. the home server's self-signed cert.
/// `HOHMANN_PIN="host:port=sha256hex"` at build time adds one more.
const BUILT_IN: &[(&str, &str)] = &[
    // hohmann-reach-server VM (LAN), self-signed cert generated 2026-10-26
    ("192.168.0.105:7777", "7f0e1c3f878beb14c583ea19a6c4cb595a81a9de49662325e1a66f7fc22892ad"),
];

pub fn fingerprint(der: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(der).iter().map(|b| format!("{b:02x}")).collect()
}

/// Split `host:port#pin` into the address and the optional pin.
pub fn split_pin(addr: &str) -> (String, Option<String>) {
    match addr.split_once('#') {
        Some((a, p)) => (a.trim().to_string(), Some(p.trim().to_lowercase().replace(':', ""))),
        None => (addr.trim().to_string(), None),
    }
}

fn pins_file() -> PathBuf {
    if let Ok(p) = std::env::var("HOHMANN_PINS") {
        return PathBuf::from(p);
    }
    crate::server_file().with_file_name("pins.txt")
}

fn built_in(addr: &str) -> Option<String> {
    let extra = option_env!("HOHMANN_PIN").and_then(|s| s.split_once('='));
    BUILT_IN.iter().copied().chain(extra).find(|(a, _)| *a == addr).map(|(_, p)| p.to_lowercase())
}

/// The remembered pin for `addr` (trust on first use).
fn stored(addr: &str) -> Option<String> {
    let text = std::fs::read_to_string(pins_file()).ok()?;
    text.lines().filter_map(|l| l.split_once(' ')).find(|(a, _)| *a == addr).map(|(_, p)| p.trim().to_string())
}

/// Remember the cert a finished handshake saw for `addr`, if it was the first time.
pub fn remember(addr: &str, first_seen: &FirstSeen) {
    if let Some(fp) = first_seen.lock().unwrap_or_else(|e| e.into_inner()).take() {
        store(addr, &fp);
    }
}

fn store(addr: &str, pin: &str) {
    // test servers on loopback get a new cert every run; don't remember those
    if addr.starts_with("127.") || addr.starts_with("localhost") || addr.starts_with("[::1]") {
        return;
    }
    let p = pins_file();
    let mut text = std::fs::read_to_string(&p).unwrap_or_default();
    text.push_str(&format!("{addr} {pin}\n"));
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, text);
}

/// Forget the remembered cert for `addr` (the player confirmed the server got a new one).
pub fn forget(addr: &str) {
    let p = pins_file();
    if let Ok(text) = std::fs::read_to_string(&p) {
        let keep: String = text.lines().filter(|l| l.split_once(' ').is_none_or(|(a, _)| a != addr)).map(|l| format!("{l}\n")).collect();
        let _ = std::fs::write(p, keep);
    }
}

#[derive(Debug)]
struct Verifier {
    addr: String,
    pin: Option<String>,
    webpki: Arc<WebPkiServerVerifier>,
    provider: Arc<CryptoProvider>,
    first_seen: FirstSeen,
}

pub const CHANGED: &str = "the server's certificate changed";

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(&self, end: &CertificateDer<'_>, inter: &[CertificateDer<'_>], name: &ServerName<'_>, ocsp: &[u8], now: UnixTime) -> Result<ServerCertVerified, Error> {
        let fp = fingerprint(end);
        let expected = self.pin.clone().or_else(|| built_in(&self.addr));
        if let Some(pin) = expected {
            // an explicit pin wins over the public roots
            return if pin == fp { Ok(ServerCertVerified::assertion()) } else { Err(Error::General(CHANGED.into())) };
        }
        if self.webpki.verify_server_cert(end, inter, name, ocsp, now).is_ok() {
            return Ok(ServerCertVerified::assertion());
        }
        match stored(&self.addr) {
            Some(pin) if pin == fp => Ok(ServerCertVerified::assertion()),
            Some(_) => Err(Error::General(CHANGED.into())),
            None => {
                *self.first_seen.lock().unwrap_or_else(|e| e.into_inner()) = Some(fp);
                Ok(ServerCertVerified::assertion())
            }
        }
    }

    fn verify_tls12_signature(&self, msg: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(msg, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(&self, msg: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(msg, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// Client config for `addr` ("host:port"), with an optional explicit pin.
pub fn config(addr: &str, pin: Option<String>) -> Result<(Arc<ClientConfig>, FirstSeen), String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let roots = RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    let webpki = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone()).build().map_err(|e| e.to_string())?;
    let first_seen = FirstSeen::default();
    let v = Verifier { addr: addr.to_string(), pin, webpki, provider: provider.clone(), first_seen: first_seen.clone() };
    let cfg = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(v))
        .with_no_client_auth();
    Ok((Arc::new(cfg), first_seen))
}

/// SNI / verification name for a "host:port" address.
pub fn server_name(addr: &str) -> Result<ServerName<'static>, String> {
    let host = match addr.rsplit_once(':') {
        Some((h, p)) if p.parse::<u16>().is_ok() => h,
        _ => addr,
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    ServerName::try_from(host.to_string()).map_err(|_| format!("bad server name {host}"))
}

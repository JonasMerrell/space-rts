//! TLS for the game port: load the configured cert, or make (once) and reuse a self-signed one.
//! Optionally a second, CA-signed cert for browsers, picked by ALPN (see `load_with_web`).
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::crypto::CryptoProvider;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::ServerConfig;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// Hex SHA-256 of a DER certificate: what the client pins.
pub fn fingerprint(der: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(der).iter().map(|b| format!("{b:02x}")).collect()
}

/// `cert`/`key` PEM files if given (e.g. Let's Encrypt fullchain.pem + privkey.pem), else
/// `<data>/tls/{cert,key}.pem`, created as a self-signed cert on first use.
pub fn load(data: &Path, cert: Option<&Path>, key: Option<&Path>) -> Result<(Arc<ServerConfig>, String), String> {
    load_with_web(data, cert, key, None)
}

/// Like `load`, plus a CA-signed cert for browsers (`web` = cert + key PEM paths): chosen when the
/// ClientHello offers ALPN http/1.1 or h2, and re-read when its file changes (certbot renewals).
pub fn load_with_web(data: &Path, cert: Option<&Path>, key: Option<&Path>, web: Option<(&Path, &Path)>) -> Result<(Arc<ServerConfig>, String), String> {
    let (cert_path, key_path) = match (cert, key) {
        (Some(c), Some(k)) => (c.to_path_buf(), k.to_path_buf()),
        (None, None) => {
            let dir = data.join("tls");
            let (c, k) = (dir.join("cert.pem"), dir.join("key.pem"));
            if !c.exists() || !k.exists() {
                make_self_signed(&dir, &c, &k)?;
            }
            (c, k)
        }
        _ => return Err("--tls-cert and --tls-key go together".into()),
    };
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(&cert_path)
        .and_then(|it| it.collect())
        .map_err(|e| format!("{}: {e}", cert_path.display()))?;
    let first = certs.first().ok_or_else(|| format!("{}: no certificate", cert_path.display()))?;
    let fp = fingerprint(first);
    let key = PrivateKeyDer::from_pem_file(&key_path).map_err(|e| format!("{}: {e}", key_path.display()))?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = ServerConfig::builder_with_provider(provider.clone()).with_safe_default_protocol_versions().map_err(|e| e.to_string())?.with_no_client_auth();
    let cfg = match web {
        None => builder.with_single_cert(certs, key).map_err(|e| format!("bad cert/key: {e}"))?,
        Some((wc, wk)) => {
            let app = Arc::new(CertifiedKey::from_der(certs, key, &provider).map_err(|e| format!("bad cert/key: {e}"))?);
            let web = WebCert { cert: wc.to_path_buf(), key: wk.to_path_buf(), provider: provider.clone(), cur: Mutex::new(None) };
            web.current().ok_or_else(|| format!("can't load the web cert {} / {}", wc.display(), wk.display()))?;
            builder.with_cert_resolver(Arc::new(Resolver { app, web }))
        }
    };
    Ok((Arc::new(cfg), fp))
}

fn read_key(cert: &Path, key: &Path, provider: &CryptoProvider) -> Result<CertifiedKey, String> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert).and_then(|it| it.collect()).map_err(|e| format!("{}: {e}", cert.display()))?;
    let key = PrivateKeyDer::from_pem_file(key).map_err(|e| format!("{}: {e}", key.display()))?;
    CertifiedKey::from_der(certs, key, provider).map_err(|e| e.to_string())
}

/// The browser cert, reloaded when the cert file's mtime changes (checked at most once a minute).
#[derive(Debug)]
struct WebCert {
    cert: PathBuf,
    key: PathBuf,
    provider: Arc<CryptoProvider>,
    /// (mtime, last check, key)
    cur: Mutex<Option<(SystemTime, Instant, Arc<CertifiedKey>)>>,
}

impl WebCert {
    fn current(&self) -> Option<Arc<CertifiedKey>> {
        let mut cur = self.cur.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((mtime, checked, k)) = cur.as_mut() {
            if checked.elapsed() < Duration::from_secs(60) {
                return Some(k.clone());
            }
            *checked = Instant::now();
            if std::fs::metadata(&self.cert).and_then(|m| m.modified()).ok() == Some(*mtime) {
                return Some(k.clone());
            }
        }
        let mtime = std::fs::metadata(&self.cert).and_then(|m| m.modified()).ok()?;
        match read_key(&self.cert, &self.key, &self.provider) {
            Ok(k) => {
                let k = Arc::new(k);
                eprintln!("loaded web TLS cert {} (sha256 {})", self.cert.display(), k.cert.first().map(|c| fingerprint(c)).unwrap_or_default());
                *cur = Some((mtime, Instant::now(), k.clone()));
                Some(k)
            }
            Err(e) => {
                // half-written renewal: keep serving the old one, try again in a minute
                eprintln!("web TLS cert: {e}");
                cur.as_ref().map(|c| c.2.clone())
            }
        }
    }
}

#[derive(Debug)]
struct Resolver {
    app: Arc<CertifiedKey>,
    web: WebCert,
}

impl ResolvesServerCert for Resolver {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let browser = hello.alpn().is_some_and(|mut p| p.any(|p| p == b"http/1.1" || p == b"h2"));
        if browser {
            self.web.current().or_else(|| Some(self.app.clone()))
        } else {
            Some(self.app.clone())
        }
    }
}

fn make_self_signed(dir: &Path, cert: &Path, key: &Path) -> Result<(), String> {
    let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "hohmanns-reach".into());
    let ck = rcgen::generate_simple_self_signed(vec![host, "hohmanns-reach".into()]).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    write_private(key, &ck.signing_key.serialize_pem())?;
    std::fs::write(cert, ck.cert.pem()).map_err(|e| format!("{}: {e}", cert.display()))?;
    eprintln!("made a self-signed TLS cert in {}", dir.display());
    Ok(())
}

fn write_private(path: &Path, data: &str) -> Result<(), String> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    o.open(path).and_then(|mut f| f.write_all(data.as_bytes())).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{ServerName, UnixTime};
    use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, ServerConnection, SignatureScheme};

    /// Accepts anything: the test only looks at which cert the server picked.
    #[derive(Debug)]
    struct Any;
    impl ServerCertVerifier for Any {
        fn verify_server_cert(&self, _: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
            Ok(ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(&self, _: &[u8], _: &CertificateDer<'_>, _: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }
        fn verify_tls13_signature(&self, _: &[u8], _: &CertificateDer<'_>, _: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }
        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            rustls::crypto::ring::default_provider().signature_verification_algorithms.supported_schemes()
        }
    }

    fn peer_cert(server: Arc<ServerConfig>, alpn: &[&[u8]]) -> String {
        let mut cc = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(Any))
            .with_no_client_auth();
        cc.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
        let mut c = ClientConnection::new(Arc::new(cc), ServerName::try_from("game.example").unwrap()).unwrap();
        let mut s = ServerConnection::new(server).unwrap();
        for _ in 0..10 {
            let mut buf = vec![];
            c.write_tls(&mut buf).unwrap();
            s.read_tls(&mut &buf[..]).unwrap();
            s.process_new_packets().unwrap();
            let mut buf = vec![];
            s.write_tls(&mut buf).unwrap();
            c.read_tls(&mut &buf[..]).unwrap();
            c.process_new_packets().unwrap();
        }
        fingerprint(&c.peer_certificates().unwrap()[0])
    }

    #[test]
    fn browsers_get_the_web_cert_apps_the_pinned_one() {
        let dir = std::env::temp_dir().join(format!("tlstest-{}", std::process::id()));
        let web = dir.join("web");
        make_self_signed(&web, &web.join("cert.pem"), &web.join("key.pem")).unwrap();
        let (cfg, app_fp) = load_with_web(&dir, None, None, Some((&web.join("cert.pem"), &web.join("key.pem")))).unwrap();
        let web_fp = {
            let pem = std::fs::read(web.join("cert.pem")).unwrap();
            fingerprint(&CertificateDer::pem_slice_iter(&pem).next().unwrap().unwrap())
        };
        assert_ne!(app_fp, web_fp);
        assert_eq!(peer_cert(cfg.clone(), &[]), app_fp, "app: no ALPN");
        assert_eq!(peer_cert(cfg.clone(), &[b"h2", b"http/1.1"]), web_fp, "browser");
        assert_eq!(peer_cert(cfg, &[b"something-else"]), app_fp);
        // without a web cert everyone gets the one cert
        let (plain, fp) = load(&dir, None, None).unwrap();
        assert_eq!(peer_cert(plain, &[b"http/1.1"]), fp);
    }
}

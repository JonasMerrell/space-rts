//! TLS for the game port: load the configured cert, or make (once) and reuse a self-signed one.
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use std::path::Path;
use std::sync::Arc;

/// Hex SHA-256 of a DER certificate: what the client pins.
pub fn fingerprint(der: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(der).iter().map(|b| format!("{b:02x}")).collect()
}

/// `cert`/`key` PEM files if given (e.g. Let's Encrypt fullchain.pem + privkey.pem), else
/// `<data>/tls/{cert,key}.pem`, created as a self-signed cert on first use.
pub fn load(data: &Path, cert: Option<&Path>, key: Option<&Path>) -> Result<(Arc<ServerConfig>, String), String> {
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
    let cfg = ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("bad cert/key: {e}"))?;
    Ok((Arc::new(cfg), fp))
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

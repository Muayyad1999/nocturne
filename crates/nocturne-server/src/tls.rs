//! TLS: certificate material as a rustls configuration, and the self-signed
//! certificate that `--tls-self-signed` keeps in the data directory.
//!
//! Everything here uses rustls with the *ring* provider, named explicitly, so
//! it does not depend on which provider a host application made the default.

use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, KeyPair, KeyUsagePurpose,
};
use rustls::client::{verify_server_cert_signed_by_trust_anchor, verify_server_name};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::server::ParsedCertificate;
use rustls::sign::CertifiedKey;
use rustls::{RootCertStore, ServerConfig};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use crate::options::Tls;
use crate::private_file;

/// Name of the self-signed certificate inside the data directory.
pub const CERT_FILE: &str = "server-cert.pem";
/// Name of its private key inside the data directory.
pub const KEY_FILE: &str = "server-key.pem";

/// Long enough not to be a nuisance, short enough for platforms that refuse
/// server certificates valid for more than 825 days.
const VALID_FOR: time::Duration = time::Duration::days(800);
/// Tolerates clients whose clock is a little behind the server's.
const BACKDATED_BY: time::Duration = time::Duration::days(1);
/// A certificate this close to its end is replaced at start rather than
/// reused, so a server that is restarted now and then never serves an expired one.
const RENEW_BEFORE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// The rustls configuration for serving HTTPS with `tls`.
pub(crate) fn server_config(tls: &Tls) -> anyhow::Result<Arc<ServerConfig>> {
    let (cert_pem, key_pem, cert_name, key_name) = match tls {
        Tls::Files { cert, key } => (
            read_pem(cert, "certificate")?,
            read_pem(key, "private key")?,
            format!("`{}`", cert.display()),
            format!("`{}`", key.display()),
        ),
        Tls::Pem { cert, key } => (
            cert.clone(),
            key.clone(),
            "the certificate PEM".to_string(),
            "the private key PEM".to_string(),
        ),
    };
    let chain = certificates(&cert_pem).with_context(|| format!("{cert_name} cannot be used"))?;
    let key = private_key(&key_pem).with_context(|| format!("{key_name} cannot be used"))?;

    let mut config = ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .context("the TLS provider offers no usable protocol version")?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .with_context(|| format!("{key_name} is not the key of the certificate in {cert_name}"))?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

fn read_pem(path: &Path, what: &str) -> anyhow::Result<String> {
    std::fs::read_to_string(path)
        .with_context(|| format!("could not read the TLS {what} `{}`", path.display()))
}

fn certificates(pem: &str) -> anyhow::Result<Vec<CertificateDer<'static>>> {
    let chain = CertificateDer::pem_slice_iter(pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| anyhow!("it is not valid PEM: {err}"))?;
    if chain.is_empty() {
        bail!(
            "it contains no certificate (a PEM block that starts with `-----BEGIN CERTIFICATE-----`)"
        );
    }
    Ok(chain)
}

fn private_key(pem: &str) -> anyhow::Result<PrivateKeyDer<'static>> {
    PrivateKeyDer::from_pem_slice(pem.as_bytes()).map_err(|err| {
        anyhow!(
            "it contains no usable private key (an unencrypted PEM block such as \
             `-----BEGIN PRIVATE KEY-----`): {err}"
        )
    })
}

/// The SHA-256 fingerprint of the first certificate in `cert_pem`, written
/// the way browsers and `openssl x509 -fingerprint` show it (`AB:CD:…`).
pub fn fingerprint(cert_pem: &str) -> anyhow::Result<String> {
    let chain = certificates(cert_pem).context("the certificate cannot be read")?;
    let digest = Sha256::digest(&chain[0]);
    Ok(digest
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":"))
}

/// A self-signed certificate and its key.
#[derive(Clone)]
pub struct SelfSigned {
    pub cert_pem: String,
    pub key_pem: String,
    /// SHA-256 of the certificate, see [`fingerprint`].
    pub fingerprint: String,
    /// Whether this call made a new certificate (`false`: the stored one was reused).
    pub created: bool,
}

impl From<SelfSigned> for Tls {
    fn from(self_signed: SelfSigned) -> Tls {
        Tls::Pem {
            cert: self_signed.cert_pem,
            key: self_signed.key_pem,
        }
    }
}

/// The self-signed certificate kept in `data_dir`, made on first use.
///
/// It is valid for `localhost`, the loopback addresses, the machine's host
/// name and `bind` (unless that is the wildcard address). The stored
/// certificate is reused for as long as it covers those names, matches its
/// key and is not about to expire; otherwise it is replaced.
pub fn self_signed(data_dir: &Path, bind: IpAddr) -> anyhow::Result<SelfSigned> {
    let cert_path = data_dir.join(CERT_FILE);
    let key_path = data_dir.join(KEY_FILE);
    let names = names_for(bind, machine_name());

    // First start, or one of the two files is gone: fall through and make a new pair.
    if let (Ok(cert_pem), Ok(key_pem)) = (
        std::fs::read_to_string(&cert_path),
        std::fs::read_to_string(&key_path),
    ) {
        match check_reusable(&cert_pem, &key_pem, &names) {
            Ok(()) => {
                if private_file::restrict(&key_path).unwrap_or(false) {
                    tracing::warn!(
                        file = %key_path.display(),
                        "the TLS private key was readable by other users; its permissions are now 0600"
                    );
                }
                return Ok(SelfSigned {
                    fingerprint: fingerprint(&cert_pem)?,
                    cert_pem,
                    key_pem,
                    created: false,
                });
            }
            Err(reason) => tracing::info!(
                %reason,
                "replacing the self-signed certificate; browsers will ask to trust the new one"
            ),
        }
    }

    let (cert_pem, key_pem) =
        generate(&names, VALID_FOR).context("could not create a self-signed certificate")?;
    // Key first: if this is interrupted, the next start sees a pair that does
    // not match and replaces it.
    private_file::replace(&key_path, key_pem.as_bytes())
        .with_context(|| format!("could not write `{}`", key_path.display()))?;
    private_file::replace(&cert_path, cert_pem.as_bytes())
        .with_context(|| format!("could not write `{}`", cert_path.display()))?;
    Ok(SelfSigned {
        fingerprint: fingerprint(&cert_pem)?,
        cert_pem,
        key_pem,
        created: true,
    })
}

/// This machine's host name, if it has one a certificate can carry.
fn machine_name() -> Option<String> {
    let name = gethostname::gethostname().into_string().ok()?;
    let name = name.trim().trim_end_matches('.').to_ascii_lowercase();
    (!name.is_empty() && ServerName::try_from(name.as_str()).is_ok()).then_some(name)
}

/// Every name the certificate must be valid for.
fn names_for(bind: IpAddr, machine_name: Option<String>) -> Vec<String> {
    let mut names = vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ];
    if !bind.is_unspecified() {
        names.push(bind.to_canonical().to_string());
    }
    names.extend(machine_name);
    let mut unique = Vec::with_capacity(names.len());
    for name in names {
        if !unique.contains(&name) {
            unique.push(name);
        }
    }
    unique
}

fn generate(names: &[String], valid_for: time::Duration) -> anyhow::Result<(String, String)> {
    let mut params = CertificateParams::new(names.to_vec())?;
    params.distinguished_name = DistinguishedName::new();
    params
        .distinguished_name
        .push(DnType::CommonName, "Nocturne server (self-signed)");
    let now = OffsetDateTime::now_utc();
    params.not_before = now - BACKDATED_BY;
    params.not_after = now + valid_for;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];

    let key = KeyPair::generate()?;
    let certificate = params.self_signed(&key)?;
    Ok((certificate.pem(), key.serialize_pem()))
}

/// Whether a stored pair can keep being used: exactly the checks a client
/// that trusts this certificate would make, for every name, now and a
/// month from now.
fn check_reusable(cert_pem: &str, key_pem: &str, names: &[String]) -> Result<(), String> {
    let chain = certificates(cert_pem).map_err(|err| format!("the stored certificate: {err}"))?;
    let key = private_key(key_pem).map_err(|err| format!("the stored key: {err}"))?;
    let provider = provider();
    CertifiedKey::from_der(chain.clone(), key, &provider).map_err(|err| {
        format!("the stored key does not belong to the stored certificate ({err})")
    })?;

    let certificate = ParsedCertificate::try_from(&chain[0])
        .map_err(|err| format!("the stored certificate cannot be parsed ({err})"))?;
    let mut trusted = RootCertStore::empty();
    trusted
        .add(chain[0].clone())
        .map_err(|err| format!("the stored certificate cannot be trusted ({err})"))?;
    let now = UnixTime::now();
    let soon = UnixTime::since_unix_epoch(Duration::from_secs(now.as_secs()) + RENEW_BEFORE);
    for (at, when) in [
        (now, "is not valid today"),
        (soon, "expires within a month"),
    ] {
        verify_server_cert_signed_by_trust_anchor(
            &certificate,
            &trusted,
            &[],
            at,
            provider.signature_verification_algorithms.all,
        )
        .map_err(|err| format!("the stored certificate {when} ({err})"))?;
    }
    for name in names {
        let server_name = ServerName::try_from(name.as_str())
            .map_err(|_| format!("`{name}` is not a name a certificate can carry"))?;
        verify_server_name(&certificate, &server_name)
            .map_err(|_| format!("the stored certificate is not valid for `{name}`"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const LOOPBACK: IpAddr = IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);

    #[test]
    fn names_cover_loopback_the_bind_address_and_the_machine() {
        assert_eq!(
            names_for(LOOPBACK, Some("noc-jump".into())),
            ["localhost", "127.0.0.1", "::1", "noc-jump"]
        );
        assert_eq!(
            names_for("0.0.0.0".parse().unwrap(), None),
            ["localhost", "127.0.0.1", "::1"]
        );
        assert_eq!(
            names_for("192.0.2.7".parse().unwrap(), Some("localhost".into())),
            ["localhost", "127.0.0.1", "::1", "192.0.2.7"]
        );
        assert_eq!(
            names_for("2001:db8::7".parse().unwrap(), Some("noc.example".into())),
            [
                "localhost",
                "127.0.0.1",
                "::1",
                "2001:db8::7",
                "noc.example"
            ]
        );
    }

    #[test]
    fn a_certificate_is_created_once_and_then_reused() {
        let dir = tempfile::tempdir().unwrap();
        let first = self_signed(dir.path(), LOOPBACK).unwrap();
        assert!(first.created);
        assert!(first.cert_pem.starts_with("-----BEGIN CERTIFICATE-----"));
        assert_eq!(
            fs::read_to_string(dir.path().join(CERT_FILE)).unwrap(),
            first.cert_pem
        );
        assert_eq!(
            fs::read_to_string(dir.path().join(KEY_FILE)).unwrap(),
            first.key_pem
        );

        let second = self_signed(dir.path(), LOOPBACK).unwrap();
        assert!(!second.created);
        assert_eq!(second.cert_pem, first.cert_pem);
        assert_eq!(second.key_pem, first.key_pem);
        assert_eq!(second.fingerprint, first.fingerprint);

        // Both can actually be served.
        server_config(&Tls::from(second)).unwrap();
        server_config(&Tls::Files {
            cert: dir.path().join(CERT_FILE),
            key: dir.path().join(KEY_FILE),
        })
        .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_private_key_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        self_signed(dir.path(), LOOPBACK).unwrap();
        let key = dir.path().join(KEY_FILE);
        assert_eq!(
            fs::metadata(&key).unwrap().permissions().mode() & 0o777,
            0o600
        );

        fs::set_permissions(&key, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!self_signed(dir.path(), LOOPBACK).unwrap().created);
        assert_eq!(
            fs::metadata(&key).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn the_certificate_is_valid_for_every_name_and_nothing_else() {
        let names = names_for("192.0.2.7".parse().unwrap(), Some("noc-jump".into()));
        let (cert_pem, key_pem) = generate(&names, VALID_FOR).unwrap();
        check_reusable(&cert_pem, &key_pem, &names).unwrap();

        let chain = certificates(&cert_pem).unwrap();
        let parsed = ParsedCertificate::try_from(&chain[0]).unwrap();
        for covered in ["localhost", "127.0.0.1", "::1", "192.0.2.7", "noc-jump"] {
            verify_server_name(&parsed, &ServerName::try_from(covered).unwrap())
                .unwrap_or_else(|err| panic!("{covered}: {err}"));
        }
        for foreign in ["example.com", "192.0.2.8", "noc-jump.example", "::2"] {
            assert!(
                verify_server_name(&parsed, &ServerName::try_from(foreign).unwrap()).is_err(),
                "{foreign}"
            );
        }
    }

    #[test]
    fn a_certificate_that_no_longer_fits_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let original = self_signed(dir.path(), LOOPBACK).unwrap();

        // Bound to another address now: the old certificate does not name it.
        let moved = self_signed(dir.path(), "192.0.2.7".parse().unwrap()).unwrap();
        assert!(moved.created);
        assert_ne!(moved.fingerprint, original.fingerprint);
        // The new one still covers loopback, so going back keeps it.
        assert!(!self_signed(dir.path(), LOOPBACK).unwrap().created);

        // A key that belongs to a different certificate.
        let (_, other_key) = generate(&names_for(LOOPBACK, None), VALID_FOR).unwrap();
        fs::write(dir.path().join(KEY_FILE), other_key).unwrap();
        assert!(self_signed(dir.path(), LOOPBACK).unwrap().created);

        // Damaged or missing files.
        fs::write(dir.path().join(CERT_FILE), "not a certificate").unwrap();
        assert!(self_signed(dir.path(), LOOPBACK).unwrap().created);
        fs::remove_file(dir.path().join(KEY_FILE)).unwrap();
        let rebuilt = self_signed(dir.path(), LOOPBACK).unwrap();
        assert!(rebuilt.created);
        server_config(&Tls::from(rebuilt)).unwrap();
    }

    #[test]
    fn a_certificate_near_its_end_is_not_reused() {
        let names = names_for(LOOPBACK, None);
        let (cert, key) = generate(&names, time::Duration::days(60)).unwrap();
        check_reusable(&cert, &key, &names).unwrap();

        let (cert, key) = generate(&names, time::Duration::days(10)).unwrap();
        let reason = check_reusable(&cert, &key, &names).unwrap_err();
        assert!(reason.contains("expires within a month"), "{reason}");

        let (cert, key) = generate(&names, time::Duration::days(-2)).unwrap();
        let reason = check_reusable(&cert, &key, &names).unwrap_err();
        assert!(reason.contains("is not valid today"), "{reason}");
    }

    #[test]
    fn the_fingerprint_is_the_sha256_of_the_der_certificate() {
        use base64::Engine;
        let (cert_pem, _) = generate(&names_for(LOOPBACK, None), VALID_FOR).unwrap();
        let printed = fingerprint(&cert_pem).unwrap();

        // Decode the PEM by hand and hash it, without the code under test.
        let base64: String = cert_pem
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect();
        let der = base64::engine::general_purpose::STANDARD
            .decode(base64)
            .unwrap();
        let expected = Sha256::digest(&der)
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(":");
        assert_eq!(printed, expected);
        assert_eq!(printed.len(), 32 * 3 - 1);
        assert!(printed.split(':').all(|pair| pair.len() == 2));
    }

    #[test]
    fn unusable_material_is_explained() {
        let (cert_pem, key_pem) = generate(&names_for(LOOPBACK, None), VALID_FOR).unwrap();
        let (_, other_key) = generate(&names_for(LOOPBACK, None), VALID_FOR).unwrap();
        let error = |cert: &str, key: &str| {
            format!(
                "{:#}",
                server_config(&Tls::Pem {
                    cert: cert.into(),
                    key: key.into()
                })
                .unwrap_err()
            )
        };
        assert!(error("", &key_pem).contains("contains no certificate"));
        // The two files swapped: a classic.
        assert!(error(&key_pem, &cert_pem).contains("contains no certificate"));
        assert!(error(&cert_pem, "garbage").contains("no usable private key"));
        assert!(error(&cert_pem, &other_key).contains("is not the key of the certificate"));

        let missing = server_config(&Tls::Files {
            cert: "/nonexistent/cert.pem".into(),
            key: "/nonexistent/key.pem".into(),
        })
        .unwrap_err();
        assert!(format!("{missing:#}").contains("/nonexistent/cert.pem"));
    }
}

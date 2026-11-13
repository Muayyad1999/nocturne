//! TLS inspection against a real, non-rustls server: `openssl s_server` with certificates
//! made by `openssl req`, and the report cross-checked with `openssl x509`.

mod support;

use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nocturne_netprobe::tls::{self, TlsReport};

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn openssl(args: &[&str]) -> String {
    let output = Command::new("openssl")
        .args(args)
        .output()
        .expect("openssl runs");
    assert!(
        output.status.success(),
        "openssl {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Makes a self-signed certificate for `interop.test` / 127.0.0.1, valid for 30 days.
fn make_certificate(dir: &Path, key_args: &[&str]) -> (String, String) {
    let key = dir.join("key.pem").display().to_string();
    let cert = dir.join("cert.pem").display().to_string();
    let mut args = vec![
        "req", "-x509", "-nodes", "-days", "30", "-keyout", &key, "-out", &cert,
    ];
    args.extend_from_slice(key_args);
    args.extend_from_slice(&[
        "-subj",
        "/C=US/O=Nocturne Interop/CN=interop.test",
        "-addext",
        "subjectAltName=DNS:interop.test,DNS:*.interop.test,IP:127.0.0.1",
    ]);
    openssl(&args);
    (key, cert)
}

/// Starts `openssl s_server` and inspects it once it listens.
async fn inspect_openssl_server(
    key: &str,
    cert: &str,
    extra: &[&str],
    sni: Option<&str>,
) -> Option<TlsReport> {
    let port = support::free_tcp_port();
    let accept = format!("127.0.0.1:{port}");
    let mut command = Command::new("openssl");
    command.args([
        "s_server", "-accept", &accept, "-key", key, "-cert", cert, "-quiet",
    ]);
    command.args(extra);
    let _server = support::Helper::spawn(&mut command).unwrap();
    for _ in 0..50 {
        match tls::inspect("127.0.0.1", port, sni, Duration::from_secs(5), unix_now()).await {
            Ok(report) => return Some(report),
            Err(nocturne_netprobe::ProbeError::Refused) => {
                tokio::time::sleep(Duration::from_millis(100)).await
            }
            Err(error) => panic!("inspecting openssl s_server failed: {error}"),
        }
    }
    eprintln!("skipping: openssl s_server did not start listening on {accept}");
    None
}

/// The value after `=` on the first line of an `openssl x509 -noout <field>` printout.
fn x509_field(cert: &str, field: &[&str]) -> String {
    let mut args = vec!["x509", "-in", cert, "-noout"];
    args.extend_from_slice(field);
    let output = openssl(&args);
    output
        .lines()
        .next()
        .unwrap()
        .split_once('=')
        .unwrap()
        .1
        .trim()
        .to_owned()
}

#[tokio::test]
async fn report_agrees_with_openssl_for_an_rsa_certificate() {
    if support::require_program("openssl").is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (key, cert) = make_certificate(dir.path(), &["-newkey", "rsa:2048"]);
    let Some(report) = inspect_openssl_server(&key, &cert, &[], Some("interop.test")).await else {
        return;
    };

    assert_eq!(report.protocol, "TLSv1.3");
    assert!(report.cipher.starts_with("TLS_"), "{}", report.cipher);
    assert!(!report.trusted, "a self-signed certificate is not trusted");
    assert!(report.hostname_matches);
    assert_eq!(report.chain.len(), 1);
    let leaf = &report.chain[0];

    assert_eq!(
        leaf.sha256_fingerprint,
        x509_field(&cert, &["-fingerprint", "-sha256"])
    );
    assert_eq!(
        leaf.serial.replace(':', ""),
        x509_field(&cert, &["-serial"])
    );
    // openssl: "C = US, O = Nocturne Interop, CN = interop.test"
    assert_eq!(
        leaf.subject,
        x509_field(&cert, &["-subject"]).replace(" = ", "=")
    );
    assert_eq!(leaf.issuer, leaf.subject);
    assert!(leaf.self_signed);
    assert_eq!(leaf.common_name, "interop.test");
    assert_eq!(leaf.sans, ["interop.test", "*.interop.test", "127.0.0.1"]);
    // openssl: "2026-12-13 03:45:00Z"
    assert_eq!(
        leaf.not_after.replace('T', " "),
        x509_field(&cert, &["-enddate", "-dateopt", "iso_8601"])
    );
    assert_eq!(
        leaf.not_before.replace('T', " "),
        x509_field(&cert, &["-startdate", "-dateopt", "iso_8601"])
    );
    assert_eq!(leaf.public_key, "RSA 2048");
    assert_eq!(leaf.signature_algorithm, "sha256WithRSAEncryption");
    // Issued a moment ago for 30 days.
    assert!((29..=30).contains(&leaf.days_left), "{}", leaf.days_left);
    assert_eq!(report.expires_in_days, leaf.days_left);
}

#[tokio::test]
async fn elliptic_curve_certificates_and_tls_1_2() {
    if support::require_program("openssl").is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (key, cert) = make_certificate(
        dir.path(),
        &["-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-384"],
    );

    let Some(report) =
        inspect_openssl_server(&key, &cert, &["-tls1_2"], Some("sw1.interop.test")).await
    else {
        return;
    };
    assert_eq!(report.protocol, "TLSv1.2");
    assert!(
        report.cipher.starts_with("TLS_ECDHE_ECDSA_WITH_"),
        "{}",
        report.cipher
    );
    assert!(
        report.hostname_matches,
        "the wildcard covers sw1.interop.test"
    );
    let leaf = &report.chain[0];
    assert_eq!(leaf.public_key, "ECDSA P-384");
    assert_eq!(leaf.signature_algorithm, "ecdsa-with-SHA256");
    assert_eq!(
        leaf.sha256_fingerprint,
        x509_field(&cert, &["-fingerprint", "-sha256"])
    );

    // The same server asked for a name the certificate does not cover, and by address.
    let Some(wrong) = inspect_openssl_server(&key, &cert, &[], Some("other.example")).await else {
        return;
    };
    assert!(!wrong.hostname_matches);
    let Some(by_address) = inspect_openssl_server(&key, &cert, &[], None).await else {
        return;
    };
    assert!(
        by_address.hostname_matches,
        "the certificate lists 127.0.0.1"
    );
}

#[tokio::test]
async fn application_protocol_negotiation_with_openssl() {
    if support::require_program("openssl").is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (key, cert) = make_certificate(dir.path(), &["-newkey", "rsa:2048"]);
    let Some(report) = inspect_openssl_server(&key, &cert, &["-alpn", "http/1.1"], None).await
    else {
        return;
    };
    assert_eq!(report.alpn.as_deref(), Some("http/1.1"));
    // A server that speaks only a protocol we do not offer: OpenSSL aborts the first handshake
    // with no_application_protocol, and the inspection retries without ALPN.
    let Some(report) = inspect_openssl_server(&key, &cert, &["-alpn", "imap"], None).await else {
        return;
    };
    assert_eq!(report.alpn, None);
    assert_eq!(report.chain.len(), 1);
}

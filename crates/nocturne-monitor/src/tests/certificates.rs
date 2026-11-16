//! TLS servers for the certificate tests, with certificates generated on the
//! spot.
//!
//! A certificate that "expires next week" cannot be a fixture: next week
//! moves. So these are made when the test runs, relative to today, which is
//! also what makes the expiry test mean anything — the check is asked about a
//! real handshake with a real certificate whose `not_after` is a date a human
//! would describe as soon.
//!
//! The certificates are self-signed, so the platform will not trust them.
//! That is deliberate: the tests that use them switch trust off
//! (`require_trusted: false`) so that what is under test is *expiry*, and the
//! separate judgement about trust is covered by the unit tests in
//! [`crate::checks::run`].

use std::sync::Arc;

use rcgen::{
    CertificateParams, DnType, ExtendedKeyUsagePurpose, KeyPair, KeyUsagePurpose, date_time_ymd,
};
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use super::listeners::Server;

/// The names the test certificates are valid for: a check aimed at
/// `127.0.0.1` sends `localhost` as its server name, exactly as
/// `nocturne-netprobe`'s own TLS tests do.
const NAMES: [&str; 2] = ["localhost", "127.0.0.1"];

/// A year-month-day triple.
type Date = (i32, u8, u8);

fn today_plus(days: i64) -> Date {
    let now = nocturne_app::util::now_ms() / 1_000;
    civil_from_days(now.div_euclid(86_400) + days)
}

/// Days since 1970-01-01 as a civil date (Howard Hinnant's algorithm, the
/// same one `nocturne-netprobe` uses to read them back).
fn civil_from_days(days: i64) -> Date {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u8, d as u8)
}

/// A TLS server whose certificate stops being valid `in_days` from now. A
/// negative number is a certificate that has already expired.
pub async fn tls_server(in_days: i64) -> Server {
    let (from, until) = if in_days < 0 {
        // Expired, and valid before that: an expired certificate, not a
        // certificate from the future.
        (today_plus(in_days - 30), today_plus(in_days))
    } else {
        (today_plus(-30), today_plus(in_days))
    };
    serve(config(from, until)).await
}

fn config(from: Date, until: Date) -> ServerConfig {
    let key = KeyPair::generate().expect("a key pair");
    let names: Vec<String> = NAMES.iter().map(|name| (*name).to_owned()).collect();
    let mut params = CertificateParams::new(names).expect("the names are usable");
    params
        .distinguished_name
        .push(DnType::OrganizationName, "Nocturne Monitor Test");
    params.distinguished_name.push(DnType::CommonName, NAMES[0]);
    params.not_before = date_time_ymd(from.0, from.1, from.2);
    params.not_after = date_time_ymd(until.0, until.1, until.2);
    // A server certificate has to say it is one, or some platform verifiers
    // refuse the chain before they consider who signed it.
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let certificate = params.self_signed(&key).expect("a certificate");
    let chain: Vec<CertificateDer<'static>> = vec![certificate.der().clone()];
    let private: PrivateKeyDer<'static> =
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
    ServerConfig::builder_with_provider(crate::crypto_provider())
        .with_safe_default_protocol_versions()
        .expect("the default protocol versions")
        .with_no_client_auth()
        .with_single_cert(chain, private)
        .expect("the certificate and key go together")
}

async fn serve(config: ServerConfig) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a listener");
    let port = listener.local_addr().expect("an address").port();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let task = tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                // The handshake is the whole service; whatever the client
                // says afterwards is read and thrown away.
                if let Ok(mut stream) = acceptor.accept(tcp).await {
                    let mut sink = Vec::new();
                    let _ = stream.read_to_end(&mut sink).await;
                }
            });
        }
    });
    Server::from_parts(port, task)
}

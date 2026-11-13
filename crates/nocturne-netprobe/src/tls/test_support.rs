//! Certificates and TLS servers for the loopback tests of `tls` and `http`.

use std::sync::Arc;

use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, date_time_ymd,
};
use rustls::ServerConfig;
use rustls::client::danger::ServerCertVerifier;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use super::{Trust, crypto_provider};

/// The moment these tests judge validity at: the real clock.
///
/// It cannot be a fixed instant. The platform verifiers these tests run
/// against — CryptoAPI on Windows, the Security framework on macOS — judge a
/// certificate against the *system* clock and cannot be told to pretend
/// otherwise. A fixture whose window sits in the past is rejected there
/// however carefully we pass a different "now" to our own code, and the
/// failure looks like a verdict about trust rather than about time. So the
/// fixtures are valid today, on whatever day the test runs, and `now()` is
/// the moment our own code is told about, agreeing with the platform because
/// it is the same moment.
pub(crate) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_secs() as i64
}

/// An RSA key generated once with `openssl genpkey` for these tests only.
const RSA_KEY_PEM: &str = include_str!("../../tests/fixtures/test-rsa-2048.key.pem");

/// A year-month-day triple.
pub(crate) type Date = (i32, u8, u8);

/// Days before `now()` a valid fixture starts, and days after it ends.
///
/// 330 days in all, comfortably inside Apple's limit: a server certificate
/// valid for more than 398 days is refused outright on macOS with
/// `"localhost" certificate is not standards compliant: -67901`, before any
/// question of trust is reached. A certificate authority may live longer
/// than that; a leaf may not.
/// A fixture that expired years ago, and stays expired: these dates are
/// fixed on purpose, because "long past" is the whole point of it.
pub(crate) const EXPIRED_FROM: Date = (2020, 1, 1);
pub(crate) const EXPIRED_UNTIL: Date = (2021, 1, 1);

const STARTED_DAYS_AGO: i64 = 30;
const ENDS_IN_DAYS: i64 = 300;

/// A day as a `Date`, `days` away from `now()`.
fn day_offset(days: i64) -> Date {
    civil_from_days(now().div_euclid(86_400) + days)
}

pub(crate) fn valid_from() -> Date {
    day_offset(-STARTED_DAYS_AGO)
}

pub(crate) fn valid_until() -> Date {
    day_offset(ENDS_IN_DAYS)
}

/// `not_before` / `not_after` as the report prints them.
pub(crate) fn date_text((year, month, day): Date) -> String {
    format!("{year:04}-{month:02}-{day:02}T00:00:00Z")
}

/// Whole days from `now()` to midnight on `date`, the way `days_left`
/// counts them — negative once it is in the past.
pub(crate) fn days_until(date: Date) -> i64 {
    (days_from_civil(date) * 86_400 - now()).div_euclid(86_400)
}

/// Whole days from `now()` to the end of a valid fixture's window, the way
/// `days_left` counts them: midnight at the far end, minus now, floored.
pub(crate) fn days_left() -> i64 {
    days_until(valid_until())
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
fn days_from_civil((year, month, day): Date) -> i64 {
    let (y, m, d) = (i64::from(year), i64::from(month), i64::from(day));
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The inverse of [`days_from_civil`].
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

/// What a test server presents: its certificate chain and key.
pub(crate) struct Identity {
    pub chain: Vec<CertificateDer<'static>>,
    pub key: PrivateKeyDer<'static>,
}

fn params(names: &[&str], common_name: &str, from: Date, until: Date) -> CertificateParams {
    let names: Vec<String> = names.iter().map(|name| (*name).to_owned()).collect();
    let mut params = CertificateParams::new(names).unwrap();
    params
        .distinguished_name
        .push(DnType::OrganizationName, "Nocturne Test");
    params
        .distinguished_name
        .push(DnType::CommonName, common_name);
    params.not_before = date_time_ymd(from.0, from.1, from.2);
    params.not_after = date_time_ymd(until.0, until.1, until.2);
    // A real server certificate says what it is for, and macOS insists on it:
    // without `serverAuth` in the extended key usage, the Security framework
    // refuses the chain with `EkuError` / "is not trusted: -67843" before it
    // considers who signed it. Eight of these tests failed there and nowhere
    // else, because webpki on Linux does not require the extension.
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params
}

fn private_key(key: &KeyPair) -> PrivateKeyDer<'static> {
    PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()))
}

/// A certificate signed by its own key, valid for `names`.
pub(crate) fn self_signed(names: &[&str], from: Date, until: Date) -> Identity {
    let key = KeyPair::generate().unwrap();
    let certificate = params(names, names[0], from, until)
        .self_signed(&key)
        .unwrap();
    Identity {
        chain: vec![certificate.der().clone()],
        key: private_key(&key),
    }
}

/// A self-signed certificate with the RSA test key.
pub(crate) fn self_signed_rsa(names: &[&str]) -> Identity {
    let key = KeyPair::from_pem(RSA_KEY_PEM).unwrap();
    let certificate = params(names, names[0], valid_from(), valid_until())
        .self_signed(&key)
        .unwrap();
    Identity {
        chain: vec![certificate.der().clone()],
        key: private_key(&key),
    }
}

/// A private certificate authority for the tests.
pub(crate) struct TestCa {
    pub certificate: CertificateDer<'static>,
    params: CertificateParams,
    key: KeyPair,
}

impl TestCa {
    pub(crate) fn new() -> Self {
        let key = KeyPair::generate().unwrap();
        let mut params = params(&[], "Nocturne Test Root CA", (2020, 1, 1), (2040, 1, 1));
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        // An authority signs certificates and revocation lists; it does not
        // serve traffic, and saying it does makes some verifiers unhappy.
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        params.extended_key_usages = Vec::new();
        let certificate = params.self_signed(&key).unwrap();
        TestCa {
            certificate: certificate.der().clone(),
            params,
            key,
        }
    }

    /// A server certificate issued by this CA. The chain holds only the leaf, as a server
    /// whose issuer is a root would send it.
    pub(crate) fn issue(&self, names: &[&str], from: Date, until: Date) -> Identity {
        let key = KeyPair::generate().unwrap();
        let issuer = Issuer::from_params(&self.params, &self.key);
        let certificate = params(names, names[0], from, until)
            .signed_by(&key, &issuer)
            .unwrap();
        Identity {
            chain: vec![certificate.der().clone()],
            key: private_key(&key),
        }
    }

    /// Trust as the platform gives it, plus this CA.
    pub(crate) fn trust(&self) -> Trust {
        let verifier = rustls_platform_verifier::Verifier::new_with_extra_roots(
            [self.certificate.clone()],
            crypto_provider(),
        )
        .unwrap();
        let verifier: Arc<dyn ServerCertVerifier> = Arc::new(verifier);
        Trust::Custom(verifier)
    }
}

/// A rustls server configuration presenting `identity`.
pub(crate) fn server_config(identity: Identity) -> ServerConfig {
    ServerConfig::builder_with_provider(crypto_provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(identity.chain, identity.key)
        .unwrap()
}

/// Starts a TLS server on loopback; `serve` gets every connection that completes its handshake.
pub(crate) async fn tls_server<F, Fut>(config: ServerConfig, serve: F) -> u16
where
    F: Fn(tokio_rustls::server::TlsStream<tokio::net::TcpStream>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let serve = Arc::new(serve);
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let (acceptor, serve) = (acceptor.clone(), serve.clone());
            tokio::spawn(async move {
                if let Ok(stream) = acceptor.accept(tcp).await {
                    serve(stream).await;
                }
            });
        }
    });
    port
}

/// Reads until the peer closes, saying nothing.
pub(crate) async fn drain(mut stream: impl AsyncRead + AsyncWrite + Unpin) {
    let mut sink = Vec::new();
    let _ = tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut sink).await;
}

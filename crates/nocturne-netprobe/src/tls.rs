//! TLS inspection: complete a handshake with any server and describe what it presented —
//! protocol, cipher, and the certificate chain — whether or not the chain can be trusted.

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::time_provider::TimeProvider;
use rustls::{
    AlertDescription, CertificateError, ClientConfig, DigitallySignedStruct, SignatureScheme,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use x509_parser::extensions::GeneralName;
use x509_parser::prelude::{FromDer, X509Certificate};
use x509_parser::public_key::PublicKey;

use crate::util::{self, format_unix_utc, millis};
use crate::{ProbeError, dns};

/// One certificate of a chain.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct CertInfo {
    /// The subject's distinguished name (`C=US, O=Example, CN=example.com`).
    pub subject: String,
    pub issuer: String,
    /// The subject's common name; empty if it has none.
    pub common_name: String,
    /// Subject alternative names: DNS names and IP addresses as they are, other kinds prefixed
    /// (`email:`, `uri:`).
    pub sans: Vec<String>,
    /// Start of validity, `YYYY-MM-DDTHH:MM:SSZ`.
    pub not_before: String,
    pub not_after: String,
    /// Whole days until `not_after`; negative once expired.
    pub days_left: i64,
    /// Serial number as colon-separated hex bytes.
    pub serial: String,
    /// `sha256WithRSAEncryption`, `ecdsa-with-SHA256`, `Ed25519`, …
    pub signature_algorithm: String,
    /// Key type and size: `RSA 2048`, `ECDSA P-256`, `Ed25519`, …
    pub public_key: String,
    pub is_ca: bool,
    /// Subject and issuer are the same.
    pub self_signed: bool,
    /// SHA-256 of the DER encoding, colon-separated upper-case hex.
    pub sha256_fingerprint: String,
}

/// What a TLS server presented.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TlsReport {
    /// The host as it was given.
    pub host: String,
    /// The address that was connected to.
    pub address: String,
    pub port: u16,
    /// `TLSv1.3` or `TLSv1.2`.
    pub protocol: String,
    /// The negotiated cipher suite by its IANA name (`TLS_AES_256_GCM_SHA384`).
    pub cipher: String,
    /// The application protocol the server selected, if any (`h2`, `http/1.1`).
    pub alpn: Option<String>,
    /// The certificates as sent: the server's own first.
    pub chain: Vec<CertInfo>,
    /// The chain leads to a root this system trusts, is within its validity period, and the
    /// server proved possession of the key. Independent of `hostname_matches`.
    pub trusted: bool,
    /// Why the chain is not trusted.
    pub trust_error: Option<String>,
    /// The server's certificate is valid for the name it was asked for (the SNI name if one
    /// was given, else the host).
    pub hostname_matches: bool,
    pub handshake_ms: f64,
    /// Days until the server's own certificate expires; negative once expired.
    pub expires_in_days: i64,
}

/// How long a set of platform trust roots is reused before it is loaded afresh.
const ROOTS_REFRESH: Duration = Duration::from_secs(600);
/// Application protocols offered when inspecting, so the report can say what the server picks.
const INSPECT_ALPN: [&str; 2] = ["h2", "http/1.1"];

/// Handshakes with `host:port` and describes the certificate chain **even when it does not
/// verify** (expired, self-signed, wrong name — that is what this is for). Trust is evaluated
/// separately, with the platform's verifier, and reported in `trusted` / `trust_error`.
///
/// `sni` overrides the server name sent and checked (default: `host`; an IP address sends
/// none). `now_unix` is the time validity is judged against, so results are reproducible.
pub async fn inspect(
    host: &str,
    port: u16,
    sni: Option<&str>,
    timeout: Duration,
    now_unix: i64,
) -> Result<TlsReport, ProbeError> {
    inspect_with(host, port, sni, timeout, now_unix, Trust::Platform).await
}

/// Where trust decisions come from.
#[derive(Clone)]
pub(crate) enum Trust {
    /// The operating system's trust store.
    Platform,
    /// A given verifier — tests use the platform verifier extended with their own root.
    #[cfg_attr(not(test), allow(dead_code))]
    Custom(Arc<dyn ServerCertVerifier>),
}

pub(crate) async fn inspect_with(
    host: &str,
    port: u16,
    sni: Option<&str>,
    timeout: Duration,
    now_unix: i64,
    trust: Trust,
) -> Result<TlsReport, ProbeError> {
    let host = util::validate_host(host)?;
    if port == 0 {
        return Err(ProbeError::invalid(
            "0 is not a TCP port; TLS services usually listen on 443",
        ));
    }
    let name = sni
        .map(str::trim)
        .filter(|sni| !sni.is_empty())
        .unwrap_or(host);
    let work = async {
        let address = SocketAddr::new(dns::resolve_one(host).await?, port);
        // Some servers abort the handshake when none of the offered application protocols is
        // theirs; they still have a certificate to show.
        let session = match connect(
            address,
            name,
            &INSPECT_ALPN,
            Mode::Inspect,
            now_unix,
            &trust,
        )
        .await
        {
            Err(Failure::AlpnRejected) => {
                connect(address, name, &[], Mode::Inspect, now_unix, &trust).await
            }
            other => other,
        };
        let (_stream, session) = session.map_err(Failure::into_probe_error)?;
        Ok::<_, ProbeError>(session.into_report(host, address))
    };
    tokio::time::timeout(timeout, work)
        .await
        .map_err(|_| ProbeError::Timeout)?
}

async fn connect(
    address: SocketAddr,
    name: &str,
    alpn: &[&str],
    mode: Mode,
    now_unix: i64,
    trust: &Trust,
) -> Result<(TlsStream<TcpStream>, Session), Failure> {
    let tcp = TcpStream::connect(address)
        .await
        .map_err(|error| Failure::Other(ProbeError::from_network_io(error)))?;
    handshake(tcp, name, alpn, mode, now_unix, trust).await
}

/// What the handshake does about certificates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Fail the handshake unless the platform trusts the certificate for the name.
    Verify,
    /// Complete the handshake whatever the certificate, then evaluate trust.
    Inspect,
}

/// Why a handshake did not complete.
#[derive(Debug)]
pub(crate) enum Failure {
    /// The server refused every application protocol offered.
    AlpnRejected,
    Other(ProbeError),
}

impl Failure {
    pub(crate) fn into_probe_error(self) -> ProbeError {
        match self {
            Failure::AlpnRejected => ProbeError::protocol(
                "the TLS server supports none of the offered application protocols",
            ),
            Failure::Other(error) => error,
        }
    }
}

/// What was negotiated and presented on one connection.
#[derive(Debug, Clone)]
pub(crate) struct Session {
    protocol: String,
    cipher: String,
    alpn: Option<String>,
    chain: Vec<CertInfo>,
    trusted: bool,
    trust_error: Option<String>,
    hostname_matches: bool,
    handshake_ms: f64,
}

impl Session {
    /// The report for a connection to `address` that was requested for `host`.
    pub(crate) fn into_report(self, host: &str, address: SocketAddr) -> TlsReport {
        TlsReport {
            host: host.to_owned(),
            address: address.ip().to_string(),
            port: address.port(),
            protocol: self.protocol,
            cipher: self.cipher,
            alpn: self.alpn,
            expires_in_days: self.chain.first().map_or(0, |leaf| leaf.days_left),
            chain: self.chain,
            trusted: self.trusted,
            trust_error: self.trust_error,
            hostname_matches: self.hostname_matches,
            handshake_ms: self.handshake_ms,
        }
    }
}

/// Performs the TLS handshake on an established connection.
///
/// With [`Mode::Verify`] this is an ordinary verified handshake. With [`Mode::Inspect`] the
/// handshake completes regardless of the certificate and the returned session says whether
/// the certificate would have been trusted — the caller must not send anything confidential
/// over such a connection.
pub(crate) async fn handshake(
    tcp: TcpStream,
    name: &str,
    alpn: &[&str],
    mode: Mode,
    now_unix: i64,
    trust: &Trust,
) -> Result<(TlsStream<TcpStream>, Session), Failure> {
    let server_name = ServerName::try_from(name.to_owned()).map_err(|_| {
        Failure::Other(ProbeError::invalid(format!(
            "'{name}' cannot be used as a TLS server name; give a DNS name or an IP address"
        )))
    })?;
    let now = UnixTime::since_unix_epoch(Duration::from_secs(u64::try_from(now_unix).unwrap_or(0)));
    let provider = crypto_provider();

    let recorder = Arc::new(Recorder {
        algorithms: provider.signature_verification_algorithms,
        signature_problem: Mutex::new(None),
    });
    let verifier: Arc<dyn ServerCertVerifier> = match mode {
        Mode::Inspect => recorder.clone(),
        Mode::Verify => trust_verifier(trust).map_err(|reason| {
            Failure::Other(ProbeError::unsupported(format!(
                "certificates cannot be verified on this machine ({reason}); install the system's \
                 CA certificates, or turn verification off to inspect the server anyway"
            )))
        })?,
    };
    let mut config = ClientConfig::builder_with_details(provider, Arc::new(FixedTime(now)))
        .with_safe_default_protocol_versions()
        .map_err(|error| Failure::Other(ProbeError::unsupported(error.to_string())))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    config.alpn_protocols = alpn
        .iter()
        .map(|protocol| protocol.as_bytes().to_vec())
        .collect();

    let started = Instant::now();
    let stream = TlsConnector::from(Arc::new(config))
        .connect(server_name.clone(), tcp)
        .await
        .map_err(|error| handshake_failure(error, name))?;
    let handshake_ms = millis(started.elapsed());

    let (_, connection) = stream.get_ref();
    let certificates: Vec<CertificateDer<'static>> = connection
        .peer_certificates()
        .map(<[CertificateDer<'static>]>::to_vec)
        .unwrap_or_default();
    let mut session = Session {
        protocol: match connection.protocol_version() {
            Some(rustls::ProtocolVersion::TLSv1_3) => "TLSv1.3".to_owned(),
            Some(rustls::ProtocolVersion::TLSv1_2) => "TLSv1.2".to_owned(),
            Some(other) => format!("{other:?}"),
            None => String::new(),
        },
        cipher: connection
            .negotiated_cipher_suite()
            .map(|suite| iana_cipher_name(suite.suite()))
            .unwrap_or_default(),
        alpn: connection
            .alpn_protocol()
            .map(|protocol| String::from_utf8_lossy(protocol).into_owned()),
        chain: certificates
            .iter()
            .map(|certificate| describe_certificate(certificate, now_unix))
            .collect(),
        trusted: true,
        trust_error: None,
        hostname_matches: true,
        handshake_ms,
    };
    if mode == Mode::Inspect {
        let signature_problem = recorder
            .signature_problem
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let trust = trust.clone();
        // Platform verifiers may consult the operating system (and, on some systems, the
        // network); keep that off the async threads.
        let verdict = tokio::task::spawn_blocking(move || {
            evaluate_trust(&certificates, &server_name, now, &trust, signature_problem)
        })
        .await
        .map_err(|error| Failure::Other(ProbeError::Io(std::io::Error::other(error))))?;
        session.trusted = verdict.trusted;
        session.trust_error = verdict.error;
        session.hostname_matches = verdict.hostname_matches;
    }
    Ok((stream, session))
}

fn handshake_failure(error: std::io::Error, name: &str) -> Failure {
    let tls_error = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<rustls::Error>());
    match tls_error {
        Some(rustls::Error::AlertReceived(AlertDescription::NoApplicationProtocol)) => {
            Failure::AlpnRejected
        }
        Some(tls_error @ rustls::Error::InvalidCertificate(_)) => {
            Failure::Other(ProbeError::protocol(format!(
                "the certificate of {name} was rejected: {}",
                describe_tls_error(tls_error)
            )))
        }
        Some(tls_error) => Failure::Other(ProbeError::protocol(format!(
            "the TLS handshake with {name} failed: {}",
            describe_tls_error(tls_error)
        ))),
        None if error.kind() == std::io::ErrorKind::UnexpectedEof => {
            Failure::Other(ProbeError::protocol(format!(
                "{name} closed the connection during the TLS handshake (is this a TLS port?)"
            )))
        }
        None => Failure::Other(ProbeError::from_network_io(error)),
    }
}

/// The process-wide crypto provider (ring), installed as rustls' default on first use.
pub(crate) fn crypto_provider() -> Arc<CryptoProvider> {
    static PROVIDER: OnceLock<Arc<CryptoProvider>> = OnceLock::new();
    PROVIDER
        .get_or_init(|| {
            // Another part of the application may have installed a provider already.
            let _ = rustls::crypto::ring::default_provider().install_default();
            Arc::new(rustls::crypto::ring::default_provider())
        })
        .clone()
}

/// Completes any handshake. It accepts every certificate and only notes whether the server's
/// handshake signature — its proof of holding the certificate's key — checked out.
///
/// This type must stay private to this module: the only way to use it is [`handshake`] in
/// [`Mode::Inspect`], which always pairs the connection with a separate trust evaluation.
#[derive(Debug)]
struct Recorder {
    algorithms: WebPkiSupportedAlgorithms,
    signature_problem: Mutex<Option<String>>,
}

impl Recorder {
    fn note(
        &self,
        outcome: Result<HandshakeSignatureValid, rustls::Error>,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        if let Err(error) = outcome {
            *self
                .signature_problem
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(describe_tls_error(&error));
        }
        Ok(HandshakeSignatureValid::assertion())
    }
}

impl ServerCertVerifier for Recorder {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.note(rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.algorithms,
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.note(rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.algorithms,
        ))
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

/// Makes rustls judge validity at the caller's `now_unix` instead of the wall clock.
#[derive(Debug)]
struct FixedTime(UnixTime);

impl TimeProvider for FixedTime {
    fn current_time(&self) -> Option<UnixTime> {
        Some(self.0)
    }
}

/// The verifier that decides trust: the platform's, or the one a test supplied.
fn trust_verifier(trust: &Trust) -> Result<Arc<dyn ServerCertVerifier>, String> {
    match trust {
        Trust::Custom(verifier) => Ok(verifier.clone()),
        Trust::Platform => platform_verifier(),
    }
}

/// The platform verifier, reloaded now and then so that changes to the trust store are seen.
fn platform_verifier() -> Result<Arc<dyn ServerCertVerifier>, String> {
    static CACHE: Mutex<Option<(Instant, Arc<rustls_platform_verifier::Verifier>)>> =
        Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((loaded, verifier)) = cache.as_ref()
        && loaded.elapsed() < ROOTS_REFRESH
    {
        return Ok(verifier.clone());
    }
    let verifier = rustls_platform_verifier::Verifier::new(crypto_provider())
        .map(Arc::new)
        .map_err(|error| error.to_string())?;
    *cache = Some((Instant::now(), verifier.clone()));
    Ok(verifier)
}

struct Verdict {
    trusted: bool,
    error: Option<String>,
    hostname_matches: bool,
}

/// Decides, after the fact, whether the chain a server presented deserves trust.
fn evaluate_trust(
    chain: &[CertificateDer<'static>],
    name: &ServerName<'static>,
    now: UnixTime,
    trust: &Trust,
    signature_problem: Option<String>,
) -> Verdict {
    let Some((end_entity, intermediates)) = chain.split_first() else {
        return Verdict {
            trusted: false,
            error: Some("the server sent no certificate".to_owned()),
            hostname_matches: false,
        };
    };
    let hostname_matches = rustls::server::ParsedCertificate::try_from(end_entity)
        .and_then(|parsed| rustls::client::verify_server_name(&parsed, name))
        .is_ok();
    let untrusted = |error: String| Verdict {
        trusted: false,
        error: Some(error),
        hostname_matches,
    };
    let verifier = match trust_verifier(trust) {
        Ok(verifier) => verifier,
        Err(reason) => {
            return untrusted(format!(
                "trust could not be evaluated on this machine: {reason}"
            ));
        }
    };

    let verify = |name: &ServerName<'_>| {
        verifier.verify_server_cert(end_entity, intermediates, name, &[], now)
    };
    let outcome = match verify(name) {
        // Trust is about the chain; whether the name fits is reported on its own. So a
        // certificate that merely belongs to another name is judged under one of its own.
        Err(error) if is_name_mismatch(&error) => own_names(end_entity)
            .iter()
            .map(verify)
            .find(|outcome| !matches!(outcome, Err(error) if is_name_mismatch(error)))
            .unwrap_or(Err(error)),
        outcome => outcome,
    };
    match (outcome, signature_problem) {
        (Err(error), _) if is_name_mismatch(&error) => untrusted(
            "the certificate is not valid for any host name (it has no usable subject alternative name)".to_owned(),
        ),
        (Err(error), _) => untrusted(describe_tls_error(&error)),
        (Ok(_), Some(problem)) => untrusted(format!(
            "the server did not prove that it holds the certificate's private key ({problem})"
        )),
        (Ok(_), None) => Verdict {
            trusted: true,
            error: None,
            hostname_matches,
        },
    }
}

fn is_name_mismatch(error: &rustls::Error) -> bool {
    matches!(
        error,
        rustls::Error::InvalidCertificate(
            CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. }
        )
    )
}

/// Server names the certificate itself claims, as names a client could ask for.
fn own_names(certificate: &CertificateDer<'_>) -> Vec<ServerName<'static>> {
    let Ok((_, parsed)) = X509Certificate::from_der(certificate) else {
        return Vec::new();
    };
    let Ok(Some(sans)) = parsed.subject_alternative_name() else {
        return Vec::new();
    };
    sans.value
        .general_names
        .iter()
        .filter_map(|name| match name {
            // "*.example.com" stands for any single label.
            GeneralName::DNSName(dns) => ServerName::try_from(dns.replacen('*', "host", 1)).ok(),
            GeneralName::IPAddress(bytes) => {
                ip_from_bytes(bytes).map(|ip| ServerName::IpAddress(ip.into()))
            }
            _ => None,
        })
        .take(8)
        .collect()
}

fn ip_from_bytes(bytes: &[u8]) -> Option<IpAddr> {
    if let Ok(v4) = <[u8; 4]>::try_from(bytes) {
        return Some(IpAddr::from(v4));
    }
    <[u8; 16]>::try_from(bytes).ok().map(IpAddr::from)
}

/// A TLS failure in words a network engineer can act on.
pub(crate) fn describe_tls_error(error: &rustls::Error) -> String {
    use rustls::Error;
    match error {
        Error::InvalidCertificate(certificate_error) => match certificate_error {
            CertificateError::Expired | CertificateError::ExpiredContext { .. } => {
                "the certificate has expired".to_owned()
            }
            CertificateError::NotValidYet | CertificateError::NotValidYetContext { .. } => {
                "the certificate is not valid yet".to_owned()
            }
            CertificateError::UnknownIssuer => {
                "the certificate is not signed by an authority this system trusts (self-signed or private CA)"
                    .to_owned()
            }
            CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. } => {
                "the certificate is not valid for this host name".to_owned()
            }
            CertificateError::Revoked => "the certificate has been revoked".to_owned(),
            CertificateError::BadSignature => "a signature in the certificate chain is invalid".to_owned(),
            CertificateError::UnsupportedSignatureAlgorithmContext { .. }
            | CertificateError::UnsupportedSignatureAlgorithmForPublicKeyContext { .. } => {
                "the certificate uses a signature algorithm or key size that is no longer accepted".to_owned()
            }
            CertificateError::BadEncoding => "the certificate is malformed".to_owned(),
            CertificateError::InvalidPurpose | CertificateError::InvalidPurposeContext { .. } => {
                "the certificate is not issued for server authentication".to_owned()
            }
            CertificateError::UnhandledCriticalExtension => {
                "the certificate has a critical extension this client does not understand".to_owned()
            }
            other => format!("the certificate was rejected ({other:?})"),
        },
        Error::AlertReceived(AlertDescription::ProtocolVersion) => {
            "the server does not support TLS 1.2 or 1.3 (it only offers older, insecure versions)".to_owned()
        }
        Error::AlertReceived(AlertDescription::HandshakeFailure) => {
            "the server rejected the handshake (no cipher suite or protocol version in common)".to_owned()
        }
        Error::AlertReceived(AlertDescription::UnrecognisedName) => {
            "the server does not serve the requested name (unrecognised SNI)".to_owned()
        }
        Error::AlertReceived(alert) => format!("the server aborted the handshake with alert {alert:?}"),
        Error::InvalidMessage(_) => "the server did not answer with TLS (is this a TLS port?)".to_owned(),
        other => other.to_string(),
    }
}

/// rustls spells TLS 1.3 suites `TLS13_…`; IANA calls them `TLS_…`.
fn iana_cipher_name(suite: rustls::CipherSuite) -> String {
    let name = suite
        .as_str()
        .map_or_else(|| format!("{suite:?}"), str::to_owned);
    match name.strip_prefix("TLS13_") {
        Some(rest) => format!("TLS_{rest}"),
        None => name,
    }
}

/// Describes one DER-encoded certificate. Never fails: what cannot be decoded is left empty.
fn describe_certificate(der: &CertificateDer<'_>, now_unix: i64) -> CertInfo {
    let sha256_fingerprint = hex_colons(&Sha256::digest(der.as_ref()));
    let Ok((_, certificate)) = X509Certificate::from_der(der.as_ref()) else {
        return CertInfo {
            subject: "(this certificate could not be decoded)".to_owned(),
            issuer: String::new(),
            common_name: String::new(),
            sans: Vec::new(),
            not_before: String::new(),
            not_after: String::new(),
            days_left: 0,
            serial: String::new(),
            signature_algorithm: String::new(),
            public_key: String::new(),
            is_ca: false,
            self_signed: false,
            sha256_fingerprint,
        };
    };
    let validity = certificate.validity();
    let not_after = validity.not_after.timestamp();
    let sans = match certificate.subject_alternative_name() {
        Ok(Some(extension)) => extension
            .value
            .general_names
            .iter()
            .map(|name| match name {
                GeneralName::DNSName(dns) => (*dns).to_owned(),
                GeneralName::IPAddress(bytes) => match ip_from_bytes(bytes) {
                    Some(address) => address.to_string(),
                    None => format!("ip:{}", hex_colons(bytes)),
                },
                GeneralName::RFC822Name(email) => format!("email:{email}"),
                GeneralName::URI(uri) => format!("uri:{uri}"),
                other => other.to_string(),
            })
            .collect(),
        _ => Vec::new(),
    };
    CertInfo {
        subject: certificate.subject().to_string(),
        issuer: certificate.issuer().to_string(),
        common_name: certificate
            .subject()
            .iter_common_name()
            .next()
            .and_then(|common_name| common_name.as_str().ok())
            .unwrap_or_default()
            .to_owned(),
        sans,
        not_before: format_unix_utc(validity.not_before.timestamp()),
        not_after: format_unix_utc(not_after),
        days_left: (not_after - now_unix).div_euclid(86_400),
        serial: hex_colons(certificate.raw_serial()),
        signature_algorithm: signature_algorithm_name(&certificate),
        public_key: public_key_description(&certificate),
        is_ca: certificate.is_ca(),
        self_signed: certificate.subject().as_raw() == certificate.issuer().as_raw(),
        sha256_fingerprint,
    }
}

fn hex_colons(bytes: &[u8]) -> String {
    let parts: Vec<String> = bytes.iter().map(|byte| format!("{byte:02X}")).collect();
    parts.join(":")
}

fn signature_algorithm_name(certificate: &X509Certificate<'_>) -> String {
    let oid = certificate.signature_algorithm.algorithm.to_id_string();
    let known = match oid.as_str() {
        "1.2.840.113549.1.1.4" => "md5WithRSAEncryption",
        "1.2.840.113549.1.1.5" => "sha1WithRSAEncryption",
        "1.2.840.113549.1.1.10" => "RSASSA-PSS",
        "1.2.840.113549.1.1.11" => "sha256WithRSAEncryption",
        "1.2.840.113549.1.1.12" => "sha384WithRSAEncryption",
        "1.2.840.113549.1.1.13" => "sha512WithRSAEncryption",
        "1.2.840.10045.4.1" => "ecdsa-with-SHA1",
        "1.2.840.10045.4.3.2" => "ecdsa-with-SHA256",
        "1.2.840.10045.4.3.3" => "ecdsa-with-SHA384",
        "1.2.840.10045.4.3.4" => "ecdsa-with-SHA512",
        "1.2.840.10040.4.3" => "dsa-with-SHA1",
        "1.3.101.112" => "Ed25519",
        "1.3.101.113" => "Ed448",
        _ => "",
    };
    if known.is_empty() {
        oid
    } else {
        known.to_owned()
    }
}

/// Key type and size, the way certificate tools print it.
fn public_key_description(certificate: &X509Certificate<'_>) -> String {
    let key_info = certificate.public_key();
    let algorithm = key_info.algorithm.algorithm.to_id_string();
    match algorithm.as_str() {
        "1.3.101.112" => return "Ed25519".to_owned(),
        "1.3.101.113" => return "Ed448".to_owned(),
        _ => {}
    }
    match key_info.parsed() {
        Ok(PublicKey::RSA(rsa)) => format!("RSA {}", bit_length(rsa.modulus)),
        Ok(PublicKey::EC(point)) => {
            let curve = key_info
                .algorithm
                .parameters
                .as_ref()
                .and_then(|parameters| parameters.as_oid().ok())
                .map(|curve| curve.to_id_string());
            match curve.as_deref() {
                Some("1.2.840.10045.3.1.7") => "ECDSA P-256".to_owned(),
                Some("1.3.132.0.34") => "ECDSA P-384".to_owned(),
                Some("1.3.132.0.35") => "ECDSA P-521".to_owned(),
                _ => format!("ECDSA {} bits", point.key_size()),
            }
        }
        Ok(PublicKey::DSA(y)) => format!("DSA {}", bit_length(y)),
        _ => format!("unknown ({algorithm})"),
    }
}

/// Bits of a big-endian unsigned integer, ignoring leading zeros.
fn bit_length(bytes: &[u8]) -> usize {
    let significant = bytes
        .iter()
        .position(|byte| *byte != 0)
        .map_or(&[][..], |start| &bytes[start..]);
    match significant.first() {
        Some(first) => significant.len() * 8 - first.leading_zeros() as usize,
        None => 0,
    }
}

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod tests {
    use super::test_support::{
        EXPIRED_FROM, EXPIRED_UNTIL, TestCa, date_text, days_left, days_until, drain, now,
        self_signed, self_signed_rsa, server_config, tls_server, valid_from, valid_until,
    };
    use super::*;
    use pretty_assertions::assert_eq;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    const TIMEOUT: Duration = Duration::from_secs(5);
    const NAMES: [&str; 2] = ["localhost", "127.0.0.1"];

    async fn inspect_local(port: u16, sni: Option<&str>, trust: Trust) -> TlsReport {
        inspect_with("127.0.0.1", port, sni, TIMEOUT, now(), trust)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn self_signed_certificate_is_described_and_not_trusted() {
        let identity = self_signed(&NAMES, valid_from(), valid_until());
        let der = identity.chain[0].clone();
        let port = tls_server(server_config(identity), drain).await;

        let report = inspect("127.0.0.1", port, Some("localhost"), TIMEOUT, now())
            .await
            .unwrap();
        assert_eq!(report.host, "127.0.0.1");
        assert_eq!(report.address, "127.0.0.1");
        assert_eq!(report.port, port);
        assert_eq!(report.protocol, "TLSv1.3");
        assert!(
            [
                "TLS_AES_256_GCM_SHA384",
                "TLS_AES_128_GCM_SHA256",
                "TLS_CHACHA20_POLY1305_SHA256"
            ]
            .contains(&report.cipher.as_str()),
            "{}",
            report.cipher
        );
        assert_eq!(report.alpn, None);
        assert!(!report.trusted);
        let reason = report.trust_error.as_deref().unwrap();
        assert!(
            reason.contains("not signed by an authority this system trusts"),
            "{reason}"
        );
        assert!(report.hostname_matches);
        assert!(report.handshake_ms > 0.0 && report.handshake_ms < 5000.0);
        assert_eq!(report.expires_in_days, days_left());

        assert_eq!(report.chain.len(), 1);
        let leaf = &report.chain[0];
        assert_eq!(leaf.subject, "CN=localhost, O=Nocturne Test");
        assert_eq!(leaf.issuer, leaf.subject);
        assert_eq!(leaf.common_name, "localhost");
        assert_eq!(leaf.sans, ["localhost", "127.0.0.1"]);
        assert_eq!(leaf.not_before, date_text(valid_from()));
        assert_eq!(leaf.not_after, date_text(valid_until()));
        assert_eq!(leaf.days_left, days_left());
        assert_eq!(leaf.signature_algorithm, "ecdsa-with-SHA256");
        assert_eq!(leaf.public_key, "ECDSA P-256");
        assert!(leaf.self_signed && !leaf.is_ca);
        assert_eq!(leaf.sha256_fingerprint, hex_colons(&Sha256::digest(&der)));
        assert_eq!(leaf.sha256_fingerprint.len(), 32 * 3 - 1);
        assert!(
            !leaf.serial.is_empty() && leaf.serial.split(':').all(|byte| byte.len() == 2),
            "{}",
            leaf.serial
        );

        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["trusted"], false);
        assert_eq!(json["chain"][0]["public_key"], "ECDSA P-256");
        assert_eq!(json["alpn"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn expired_certificate_is_still_inspected() {
        let identity = self_signed(&NAMES, EXPIRED_FROM, EXPIRED_UNTIL);
        let port = tls_server(server_config(identity), drain).await;
        let report = inspect("127.0.0.1", port, Some("localhost"), TIMEOUT, now())
            .await
            .unwrap();
        assert!(!report.trusted);
        assert!(report.trust_error.is_some());
        let leaf = &report.chain[0];
        assert_eq!(leaf.not_after, date_text(EXPIRED_UNTIL));
        // 2021-01-01 to 2026-01-01 is 1826 days ago.
        // Long expired, counted from today rather than from a number that
        // was true on the day it was written.
        assert_eq!(leaf.days_left, days_until(EXPIRED_UNTIL));
        assert_eq!(report.expires_in_days, days_until(EXPIRED_UNTIL));
        assert!(leaf.days_left < -1_000, "{}", leaf.days_left);

        // The day it expires, to the second.
        let last_second = 1_609_459_199;
        let report = inspect("127.0.0.1", port, Some("localhost"), TIMEOUT, last_second)
            .await
            .unwrap();
        assert_eq!(report.expires_in_days, 0);
        let report = inspect(
            "127.0.0.1",
            port,
            Some("localhost"),
            TIMEOUT,
            last_second + 2,
        )
        .await
        .unwrap();
        assert_eq!(report.expires_in_days, -1);
    }

    #[tokio::test]
    async fn chain_from_a_trusted_authority_is_trusted() {
        let ca = TestCa::new();
        let port = tls_server(
            server_config(ca.issue(&NAMES, valid_from(), valid_until())),
            drain,
        )
        .await;
        let report = inspect_local(port, Some("localhost"), ca.trust()).await;
        assert!(report.trusted, "{:?}", report.trust_error);
        assert_eq!(report.trust_error, None);
        assert!(report.hostname_matches);
        let leaf = &report.chain[0];
        assert_eq!(leaf.issuer, "CN=Nocturne Test Root CA, O=Nocturne Test");
        assert!(!leaf.self_signed && !leaf.is_ca);

        // The same server without the test root in the trust store.
        let report = inspect("127.0.0.1", port, Some("localhost"), TIMEOUT, now())
            .await
            .unwrap();
        assert!(!report.trusted);
    }

    #[tokio::test]
    async fn validity_is_judged_at_the_given_time() {
        let ca = TestCa::new();
        let port = tls_server(
            server_config(ca.issue(&NAMES, valid_from(), valid_until())),
            drain,
        )
        .await;
        let check = |now: i64| {
            inspect_with(
                "127.0.0.1",
                port,
                Some("localhost"),
                TIMEOUT,
                now,
                ca.trust(),
            )
        };

        let before = check(1_700_000_000).await.unwrap(); // November 2023
        assert!(!before.trusted);
        assert_eq!(
            before.trust_error.as_deref(),
            Some("the certificate is not valid yet")
        );

        let during = check(now()).await.unwrap();
        assert!(during.trusted);

        let after = check(1_900_000_000).await.unwrap(); // March 2030
        assert!(!after.trusted);
        assert_eq!(
            after.trust_error.as_deref(),
            Some("the certificate has expired")
        );
        assert!(after.expires_in_days < 0);
    }

    #[tokio::test]
    async fn a_wrong_name_is_reported_separately_from_trust() {
        let ca = TestCa::new();
        let port = tls_server(
            server_config(ca.issue(&NAMES, valid_from(), valid_until())),
            drain,
        )
        .await;
        let report = inspect_local(port, Some("intranet.example"), ca.trust()).await;
        assert!(!report.hostname_matches);
        assert!(
            report.trusted,
            "the chain itself is fine: {:?}",
            report.trust_error
        );

        // Without SNI the address is what must match; this certificate lists it.
        let report = inspect_local(port, None, ca.trust()).await;
        assert!(report.hostname_matches && report.trusted);

        // A wildcard certificate asked for the wrong domain.
        let wildcard = ca.issue(&["*.lab.example"], valid_from(), valid_until());
        let port = tls_server(server_config(wildcard), drain).await;
        let report = inspect_local(port, Some("sw1.lab.example"), ca.trust()).await;
        assert!(report.hostname_matches && report.trusted);
        let report = inspect_local(port, Some("sw1.other.example"), ca.trust()).await;
        assert!(!report.hostname_matches);
        assert!(report.trusted, "{:?}", report.trust_error);
        assert_eq!(report.chain[0].sans, ["*.lab.example"]);
    }

    #[tokio::test]
    async fn the_whole_chain_the_server_sends_is_listed() {
        let ca = TestCa::new();
        let mut identity = ca.issue(&NAMES, valid_from(), valid_until());
        identity.chain.push(ca.certificate.clone());
        let port = tls_server(server_config(identity), drain).await;
        let report = inspect_local(port, Some("localhost"), ca.trust()).await;
        assert_eq!(report.chain.len(), 2);
        let root = &report.chain[1];
        assert_eq!(root.common_name, "Nocturne Test Root CA");
        assert!(root.is_ca && root.self_signed);
        assert_eq!(root.sans, Vec::<String>::new());
        assert_eq!(root.not_after, "2040-01-01T00:00:00Z");
        // Expiry is the server certificate's, not the root's.
        assert_eq!(report.expires_in_days, days_left());
    }

    #[tokio::test]
    async fn rsa_certificates_are_described() {
        let port = tls_server(server_config(self_signed_rsa(&NAMES)), drain).await;
        let report = inspect("127.0.0.1", port, Some("localhost"), TIMEOUT, now())
            .await
            .unwrap();
        let leaf = &report.chain[0];
        assert_eq!(leaf.public_key, "RSA 2048");
        assert_eq!(leaf.signature_algorithm, "sha256WithRSAEncryption");
    }

    #[tokio::test]
    async fn tls_1_2_servers_are_reported_as_such() {
        let identity = self_signed(&NAMES, valid_from(), valid_until());
        let config = ServerConfigBuilder::tls12_only(identity);
        let port = tls_server(config, drain).await;
        let report = inspect("127.0.0.1", port, Some("localhost"), TIMEOUT, now())
            .await
            .unwrap();
        assert_eq!(report.protocol, "TLSv1.2");
        assert!(
            report.cipher.starts_with("TLS_ECDHE_ECDSA_WITH_"),
            "{}",
            report.cipher
        );
    }

    /// Server configurations the default builder does not produce.
    struct ServerConfigBuilder;

    impl ServerConfigBuilder {
        fn tls12_only(identity: super::test_support::Identity) -> rustls::ServerConfig {
            rustls::ServerConfig::builder_with_provider(crypto_provider())
                .with_protocol_versions(&[&rustls::version::TLS12])
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(identity.chain, identity.key)
                .unwrap()
        }
    }

    #[tokio::test]
    async fn the_application_protocol_the_server_selects_is_reported() {
        let mut config = server_config(self_signed(&NAMES, valid_from(), valid_until()));
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        let port = tls_server(config, drain).await;
        let report = inspect("127.0.0.1", port, None, TIMEOUT, now())
            .await
            .unwrap();
        assert_eq!(report.alpn.as_deref(), Some("http/1.1"));

        let mut config = server_config(self_signed(&NAMES, valid_from(), valid_until()));
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let port = tls_server(config, drain).await;
        let report = inspect("127.0.0.1", port, None, TIMEOUT, now())
            .await
            .unwrap();
        assert_eq!(report.alpn.as_deref(), Some("h2"));
    }

    #[tokio::test]
    async fn a_server_that_rejects_the_offered_protocols_is_inspected_without_them() {
        // An IMAPS-style server: it insists on its own application protocol.
        let mut config = server_config(self_signed(&NAMES, valid_from(), valid_until()));
        config.alpn_protocols = vec![b"imap".to_vec()];
        let port = tls_server(config, drain).await;
        let report = inspect("127.0.0.1", port, Some("localhost"), TIMEOUT, now())
            .await
            .unwrap();
        assert_eq!(report.alpn, None);
        assert_eq!(report.chain.len(), 1);
    }

    #[tokio::test]
    async fn verified_handshakes_accept_only_trusted_matching_certificates() {
        let ca = TestCa::new();
        let trust = ca.trust();
        let attempt = |identity, name: &'static str, now: i64| {
            let trust = trust.clone();
            async move {
                let port = tls_server(server_config(identity), drain).await;
                let address: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
                connect(address, name, &["http/1.1"], Mode::Verify, now, &trust)
                    .await
                    .map(|(_, session)| session)
                    .map_err(|failure| failure.into_probe_error().to_string())
            }
        };

        let session = attempt(
            ca.issue(&NAMES, valid_from(), valid_until()),
            "localhost",
            now(),
        )
        .await
        .unwrap();
        assert!(session.trusted && session.hostname_matches);
        assert_eq!(session.chain.len(), 1);

        let error = attempt(
            self_signed(&NAMES, valid_from(), valid_until()),
            "localhost",
            now(),
        )
        .await
        .unwrap_err();
        assert!(
            error.contains("certificate of localhost was rejected"),
            "{error}"
        );
        assert!(
            error.contains("not signed by an authority this system trusts"),
            "{error}"
        );

        let error = attempt(
            ca.issue(&NAMES, valid_from(), valid_until()),
            "localhost",
            1_900_000_000,
        )
        .await
        .unwrap_err();
        assert!(error.contains("has expired"), "{error}");

        let error = attempt(
            ca.issue(&NAMES, valid_from(), valid_until()),
            "other.example",
            now(),
        )
        .await
        .unwrap_err();
        assert!(error.contains("not valid for this host name"), "{error}");
    }

    #[tokio::test]
    async fn things_that_are_not_tls_servers() {
        // A plain-text service.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let _ = stream.write_all(b"220 mail.example ESMTP ready\r\n").await;
                drain(stream).await;
            }
        });
        let error = inspect("127.0.0.1", port, None, TIMEOUT, now())
            .await
            .unwrap_err();
        assert!(
            matches!(&error, ProbeError::Protocol(text) if text.contains("TLS")),
            "{error:?}"
        );

        // A service that hangs up at once.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let hangup = listener.local_addr().unwrap().port();
        tokio::spawn(async move { while let Ok((_stream, _)) = listener.accept().await {} });
        let error = inspect("127.0.0.1", hangup, None, TIMEOUT, now())
            .await
            .unwrap_err();
        assert!(
            matches!(error, ProbeError::Protocol(_) | ProbeError::Io(_)),
            "{error:?}"
        );

        // Nobody listening.
        let closed = {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            listener.local_addr().unwrap().port()
        };
        let error = inspect("127.0.0.1", closed, None, TIMEOUT, now())
            .await
            .unwrap_err();
        assert!(matches!(error, ProbeError::Refused), "{error:?}");

        // A server that accepts and stays silent.
        let silent = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = silent.local_addr().unwrap().port();
        let started = Instant::now();
        let error = inspect("127.0.0.1", port, None, Duration::from_millis(300), now())
            .await
            .unwrap_err();
        assert!(matches!(error, ProbeError::Timeout), "{error:?}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn bad_input_is_rejected() {
        for (host, port, sni) in [
            ("", 443, None),
            ("127.0.0.1", 0, None),
            ("two words", 443, None),
            ("127.0.0.1", 443, Some("not a name")),
        ] {
            let error = inspect(host, port, sni, TIMEOUT, now()).await.unwrap_err();
            assert!(
                matches!(error, ProbeError::Invalid(_) | ProbeError::Refused),
                "{host:?}:{port} {sni:?}: {error:?}"
            );
        }
        assert!(matches!(
            inspect("no-such-host.invalid", 443, None, TIMEOUT, now()).await,
            Err(ProbeError::Resolve)
        ));
    }

    #[test]
    fn undecodable_certificates_never_panic() {
        let placeholder = describe_certificate(
            &CertificateDer::from(vec![0x30, 0x03, 0x02, 0x01, 0x01]),
            now(),
        );
        assert_eq!(
            placeholder.subject,
            "(this certificate could not be decoded)"
        );
        assert_eq!(placeholder.sha256_fingerprint.len(), 95);

        let good = self_signed(&NAMES, valid_from(), valid_until())
            .chain
            .remove(0);
        for cut in 0..good.len() {
            let _ = describe_certificate(&CertificateDer::from(good[..cut].to_vec()), now());
            let _ = own_names(&CertificateDer::from(good[..cut].to_vec()));
        }
        let mut state = 0xdead_beef_cafe_f00du64;
        for _ in 0..2000 {
            let mut mutated = good.to_vec();
            for _ in 0..4 {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let index = (state % mutated.len() as u64) as usize;
                mutated[index] = (state >> 32) as u8;
            }
            let _ = describe_certificate(&CertificateDer::from(mutated.clone()), now());
            let _ = own_names(&CertificateDer::from(mutated));
        }
    }

    #[test]
    fn small_helpers() {
        assert_eq!(bit_length(&[0x00, 0x80, 0x00]), 16);
        assert_eq!(bit_length(&[0x01, 0x00]), 9);
        assert_eq!(bit_length(&[0x00, 0x00]), 0);
        assert_eq!(bit_length(&[]), 0);
        assert_eq!(hex_colons(&[0x00, 0xab, 0x10]), "00:AB:10");
        assert_eq!(hex_colons(&[]), "");
        assert_eq!(
            iana_cipher_name(rustls::CipherSuite::TLS13_AES_256_GCM_SHA384),
            "TLS_AES_256_GCM_SHA384"
        );
        assert_eq!(
            iana_cipher_name(rustls::CipherSuite::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256),
            "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256"
        );
        assert_eq!(
            ip_from_bytes(&[10, 0, 0, 1]),
            Some("10.0.0.1".parse().unwrap())
        );
        assert_eq!(ip_from_bytes(&[0; 16]), Some("::".parse().unwrap()));
        assert_eq!(ip_from_bytes(&[1, 2, 3]), None);
    }
}

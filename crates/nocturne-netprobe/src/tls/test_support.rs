//! Certificates and TLS servers for the loopback tests of `tls` and `http`.

use std::sync::Arc;

use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, date_time_ymd};
use rustls::ServerConfig;
use rustls::client::danger::ServerCertVerifier;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use super::{Trust, crypto_provider};

/// 2026-01-01T00:00:00Z: the "now" every test judges validity against.
pub(crate) const NOW: i64 = 1_767_225_600;
/// An RSA key generated once with `openssl genpkey` for these tests only.
const RSA_KEY_PEM: &str = include_str!("../../tests/fixtures/test-rsa-2048.key.pem");

/// A year-month-day triple.
pub(crate) type Date = (i32, u8, u8);
pub(crate) const VALID_FROM: Date = (2024, 1, 1);
pub(crate) const VALID_UNTIL: Date = (2030, 1, 1);

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
    let certificate = params(names, names[0], VALID_FROM, VALID_UNTIL)
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

//! Sending syslog: one collector, over UDP, TCP or TLS.
//!
//! Syslog has no acknowledgement on any of its transports, so "sent" only ever means "handed
//! to the network". What a sender can still do is not lie about the failures it *can* see:
//! a connection the collector closed is noticed before the next batch is written into it, a
//! collector that has stopped reading is an error after the write timeout rather than a task
//! that never returns, and a certificate nobody vouches for ends the attempt before a byte of
//! log text is written.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::ClientConfig;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};

use crate::ProbeError;

/// How long a UDP socket is used before the collector's name is looked up again, so that a
/// collector whose address changed is followed without anyone restarting anything.
const UDP_REFRESH: Duration = Duration::from_secs(300);
/// An idle TCP connection is probed after this long, so that a collector that vanished
/// without closing it is noticed by the operating system rather than by lost messages.
const KEEPALIVE_AFTER: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Udp,
    Tcp,
    Tls,
}

/// How messages are told apart on a stream (RFC 6587). A datagram needs none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Framing {
    /// One message to a line. A line break inside a message is written as a space.
    Newline,
    /// `<length> <message>`: what RFC 5425 requires over TLS.
    OctetCounted,
}

/// Whose word a TLS collector's certificate is checked against.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Trust {
    /// The authorities this machine trusts.
    #[default]
    Platform,
    /// These authorities and no others: a collector with a certificate from a private CA.
    Authorities(Vec<CertificateDer<'static>>),
}

/// The certificates in a PEM text, for [`Trust::Authorities`].
pub fn authorities_from_pem(pem: &str) -> Result<Vec<CertificateDer<'static>>, ProbeError> {
    let certificates = CertificateDer::pem_slice_iter(pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ProbeError::invalid("the certificate is not valid PEM"))?;
    if certificates.is_empty() {
        return Err(ProbeError::invalid(
            "there is no certificate in this text; paste the authority's certificate in PEM \
             form, `-----BEGIN CERTIFICATE-----` included",
        ));
    }
    // Reading them into a trust store is what tells a certificate from other base64.
    let mut roots = rustls::RootCertStore::empty();
    for certificate in &certificates {
        roots.add(certificate.clone()).map_err(|_| {
            ProbeError::invalid("the PEM holds something that is not a certificate")
        })?;
    }
    Ok(certificates)
}

/// Where the messages go, and how.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// A host name or an address.
    pub host: String,
    pub port: u16,
    pub transport: Transport,
    /// Ignored for UDP.
    pub framing: Framing,
    /// Ignored unless the transport is TLS.
    pub trust: Trust,
    /// For the name lookup, the TCP connection and the TLS handshake, each.
    pub connect_timeout: Duration,
    /// How long one batch may take to be written. A collector that takes nothing for this
    /// long has stopped reading.
    pub write_timeout: Duration,
}

impl Target {
    /// A target with the framing its transport calls for — octet counting over TLS
    /// (RFC 5425), a message to a line over TCP — and ten seconds for each timeout.
    pub fn new(host: impl Into<String>, port: u16, transport: Transport) -> Target {
        Target {
            host: host.into(),
            port,
            transport,
            framing: match transport {
                Transport::Tls => Framing::OctetCounted,
                Transport::Udp | Transport::Tcp => Framing::Newline,
            },
            trust: Trust::Platform,
            connect_timeout: Duration::from_secs(10),
            write_timeout: Duration::from_secs(10),
        }
    }
}

/// A batch that did not all leave.
#[derive(Debug)]
pub struct Failed {
    /// How many of the messages are known to have been handed to the network. On a stream
    /// this is 0: what the connection had already taken when it failed cannot be known.
    pub sent: usize,
    pub error: ProbeError,
}

impl Failed {
    fn nothing(error: ProbeError) -> Failed {
        Failed { sent: 0, error }
    }
}

// `Sync` so that a sender can be held across an await by a task that may move threads.
trait Wire: AsyncRead + AsyncWrite + Send + Sync + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Sync + Unpin> Wire for T {}

enum Link {
    Datagram { socket: UdpSocket, since: Instant },
    Stream(Box<dyn Wire>),
}

/// A sender for one collector. It connects when there is something to send and again after
/// a failure; it never retries by itself.
pub struct SyslogSender {
    target: Target,
    link: Option<Link>,
}

impl SyslogSender {
    /// Checks the target; connects nothing.
    pub fn new(target: Target) -> Result<SyslogSender, ProbeError> {
        crate::util::validate_host(&target.host)?;
        if target.port == 0 {
            return Err(ProbeError::invalid(
                "port 0 is not a port a collector can listen on",
            ));
        }
        if target.transport == Transport::Tls {
            // A name that cannot be checked against a certificate is refused now, not at
            // the first message.
            server_name(&target.host)?;
        }
        Ok(SyslogSender { target, link: None })
    }

    pub fn target(&self) -> &Target {
        &self.target
    }

    pub fn is_connected(&self) -> bool {
        self.link.is_some()
    }

    /// Lets the connection go. The next [`send`](Self::send) opens a new one.
    pub fn disconnect(&mut self) {
        self.link = None;
    }

    /// Sends the messages in order; each is one syslog message without framing.
    ///
    /// Over UDP every message is tried even when one fails, and the error says how many
    /// left. Over a stream the batch is written as one piece: after a failure the
    /// connection is gone, an unknown part of the batch may have arrived, and sending it
    /// again on a new connection may deliver that part twice.
    pub async fn send(&mut self, messages: &[Vec<u8>]) -> Result<(), Failed> {
        if messages.is_empty() {
            return Ok(());
        }
        match self.target.transport {
            Transport::Udp => self.send_datagrams(messages).await,
            Transport::Tcp | Transport::Tls => self.send_stream(messages).await,
        }
    }

    async fn send_datagrams(&mut self, messages: &[Vec<u8>]) -> Result<(), Failed> {
        let stale = matches!(&self.link, Some(Link::Datagram { since, .. }) if since.elapsed() > UDP_REFRESH);
        if stale {
            self.link = None;
        }
        if self.link.is_none() {
            let socket = self.open_datagram().await.map_err(Failed::nothing)?;
            self.link = Some(Link::Datagram {
                socket,
                since: Instant::now(),
            });
        }
        let Some(Link::Datagram { socket, .. }) = &self.link else {
            unreachable!("a datagram link was just opened");
        };
        let mut sent = 0;
        let mut failure = None;
        for message in messages {
            match socket.send(message).await {
                Ok(_) => sent += 1,
                // A closed port answers the *previous* datagram with ICMP, and the socket
                // reports it on this one. The rest are still tried: the next may be the
                // one that gets through after the collector came back.
                Err(error) => failure = Some(ProbeError::from_datagram_io(error)),
            }
        }
        match failure {
            None => Ok(()),
            Some(error) => {
                // Look the name up again next time: the address may be what is wrong.
                self.link = None;
                Err(Failed { sent, error })
            }
        }
    }

    async fn open_datagram(&self) -> Result<UdpSocket, ProbeError> {
        let addresses = self.resolve().await?;
        let mut last = ProbeError::Resolve;
        for address in addresses {
            let local: SocketAddr = if address.is_ipv4() {
                (std::net::Ipv4Addr::UNSPECIFIED, 0).into()
            } else {
                (std::net::Ipv6Addr::UNSPECIFIED, 0).into()
            };
            let opened = async {
                let socket = UdpSocket::bind(local).await?;
                socket.connect(address).await?;
                Ok::<_, io::Error>(socket)
            };
            match opened.await {
                Ok(socket) => return Ok(socket),
                Err(error) => last = ProbeError::from_network_io(error),
            }
        }
        Err(last)
    }

    async fn send_stream(&mut self, messages: &[Vec<u8>]) -> Result<(), Failed> {
        let mut batch = Vec::with_capacity(messages.iter().map(|message| message.len() + 12).sum());
        for message in messages {
            frame(&mut batch, message, self.target.framing);
        }

        // A collector never says anything, so the only thing there can be to read on an
        // open connection is its end. Written into a connection the collector has closed,
        // a batch is accepted by the operating system and lost without an error.
        if let Some(Link::Stream(stream)) = &mut self.link
            && peer_gone(stream.as_mut()).await
        {
            self.link = None;
        }
        if self.link.is_none() {
            let stream = self.open_stream().await.map_err(Failed::nothing)?;
            self.link = Some(Link::Stream(stream));
        }
        let Some(Link::Stream(stream)) = &mut self.link else {
            unreachable!("a stream link was just opened");
        };
        let written = tokio::time::timeout(self.target.write_timeout, async {
            stream.write_all(&batch).await?;
            stream.flush().await
        })
        .await;
        let error = match written {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(error)) => match error.kind() {
                io::ErrorKind::BrokenPipe
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::ConnectionAborted
                | io::ErrorKind::UnexpectedEof
                | io::ErrorKind::WriteZero => ProbeError::Io(io::Error::new(
                    error.kind(),
                    "the collector closed the connection",
                )),
                _ => ProbeError::from_network_io(error),
            },
            Err(_) => ProbeError::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "the collector stopped reading: nothing more could be written for {} s",
                    self.target.write_timeout.as_secs_f32()
                ),
            )),
        };
        // Part of a frame may be on the wire; nothing after it could be told apart.
        self.link = None;
        Err(Failed::nothing(error))
    }

    async fn open_stream(&self) -> Result<Box<dyn Wire>, ProbeError> {
        let addresses = self.resolve().await?;
        let mut last = ProbeError::Resolve;
        let mut connected = None;
        for address in addresses {
            match tokio::time::timeout(self.target.connect_timeout, TcpStream::connect(address))
                .await
            {
                Ok(Ok(stream)) => {
                    connected = Some(stream);
                    break;
                }
                Ok(Err(error)) => last = ProbeError::from_network_io(error),
                Err(_) => last = ProbeError::Timeout,
            }
        }
        let Some(tcp) = connected else {
            return Err(last);
        };
        // Batches are written whole and flushed; waiting to coalesce them only delays logs.
        let _ = tcp.set_nodelay(true);
        let keepalive = socket2::TcpKeepalive::new().with_time(KEEPALIVE_AFTER);
        let _ = socket2::SockRef::from(&tcp).set_tcp_keepalive(&keepalive);
        if self.target.transport == Transport::Tcp {
            return Ok(Box::new(tcp));
        }

        let name = server_name(&self.target.host)?;
        let config = client_config(&self.target.trust)?;
        let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
        match tokio::time::timeout(self.target.connect_timeout, connector.connect(name, tcp)).await
        {
            Ok(Ok(stream)) => Ok(Box::new(stream)),
            Ok(Err(error)) => Err(handshake_error(error)),
            Err(_) => Err(ProbeError::Timeout),
        }
    }

    async fn resolve(&self) -> Result<Vec<SocketAddr>, ProbeError> {
        let host = self.target.host.trim();
        if let Some(address) = crate::util::parse_ip_literal(host) {
            return Ok(vec![SocketAddr::new(address, self.target.port)]);
        }
        let lookup = tokio::net::lookup_host((host, self.target.port));
        match tokio::time::timeout(self.target.connect_timeout, lookup).await {
            Ok(Ok(addresses)) => {
                let addresses: Vec<SocketAddr> = addresses.collect();
                if addresses.is_empty() {
                    Err(ProbeError::Resolve)
                } else {
                    Ok(addresses)
                }
            }
            Ok(Err(_)) => Err(ProbeError::Resolve),
            Err(_) => Err(ProbeError::Timeout),
        }
    }
}

/// Appends one message to a stream batch.
fn frame(batch: &mut Vec<u8>, message: &[u8], framing: Framing) {
    match framing {
        Framing::OctetCounted => {
            batch.extend_from_slice(message.len().to_string().as_bytes());
            batch.push(b' ');
            batch.extend_from_slice(message);
        }
        Framing::Newline => {
            // Whatever the caller formatted, a line end inside it would be read as the end
            // of this message and the start of one nobody sent.
            batch.extend(message.iter().map(|byte| match byte {
                b'\n' | b'\r' => b' ',
                byte => *byte,
            }));
            batch.push(b'\n');
        }
    }
}

/// Whether the other end has closed the connection.
async fn peer_gone(stream: &mut dyn Wire) -> bool {
    let mut scratch = [0u8; 512];
    // Bounded, so a peer that keeps talking cannot keep the sender here.
    for _ in 0..8 {
        tokio::select! {
            biased;
            read = stream.read(&mut scratch) => match read {
                Ok(0) | Err(_) => return true,
                // Something was said that nobody asked for. It is not the end.
                Ok(_) => {}
            },
            // Nothing to read right now: the connection is open.
            _ = std::future::ready(()) => return false,
        }
    }
    false
}

fn server_name(host: &str) -> Result<ServerName<'static>, ProbeError> {
    let host = host.trim();
    let name = match crate::util::parse_ip_literal(host) {
        Some(address) => ServerName::IpAddress(address.into()),
        None => ServerName::try_from(host.to_owned()).map_err(|_| {
            ProbeError::invalid(format!(
                "'{host}' is not a name a certificate can be checked against"
            ))
        })?,
    };
    Ok(name)
}

fn client_config(trust: &Trust) -> Result<ClientConfig, ProbeError> {
    let provider = crate::tls::crypto_provider();
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|error| ProbeError::unsupported(format!("TLS is not available: {error}")))?;
    Ok(match trust {
        Trust::Platform => {
            let verifier = rustls_platform_verifier::Verifier::new(provider).map_err(|error| {
                ProbeError::unsupported(format!(
                    "this machine's trusted authorities could not be read: {error}"
                ))
            })?;
            builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(verifier))
                .with_no_client_auth()
        }
        Trust::Authorities(certificates) => {
            let mut roots = rustls::RootCertStore::empty();
            for certificate in certificates {
                roots.add(certificate.clone()).map_err(|_| {
                    ProbeError::invalid(
                        "the authority given for this collector is not a certificate",
                    )
                })?;
            }
            builder.with_root_certificates(roots).with_no_client_auth()
        }
    })
}

fn handshake_error(error: io::Error) -> ProbeError {
    let reason = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<rustls::Error>())
        .map(crate::tls::describe_tls_error);
    match reason {
        Some(reason) => ProbeError::protocol(format!(
            "the TLS handshake with the collector failed: {reason}"
        )),
        None if error.kind() == io::ErrorKind::UnexpectedEof => ProbeError::protocol(
            "the collector closed the connection during the TLS handshake (is this a TLS port?)",
        ),
        None => ProbeError::from_network_io(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tls::test_support::{TestCa, server_config, tls_server, valid_from, valid_until};
    use pretty_assertions::assert_eq;
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    fn messages(texts: &[&str]) -> Vec<Vec<u8>> {
        texts.iter().map(|text| text.as_bytes().to_vec()).collect()
    }

    fn quick(mut target: Target) -> Target {
        target.connect_timeout = Duration::from_secs(5);
        target.write_timeout = Duration::from_secs(5);
        target
    }

    /// A TCP listener that hands every connection's bytes, as they arrive, to the test.
    async fn tcp_collector() -> (u16, mpsc::UnboundedReceiver<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (seen, receiver) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let seen = seen.clone();
                tokio::spawn(async move {
                    let mut chunk = vec![0u8; 4096];
                    while let Ok(count) = stream.read(&mut chunk).await {
                        if count == 0 || seen.send(chunk[..count].to_vec()).is_err() {
                            break;
                        }
                    }
                });
            }
        });
        (port, receiver)
    }

    /// Everything received until `wanted` bytes are there, or two seconds have passed.
    async fn collect(receiver: &mut mpsc::UnboundedReceiver<Vec<u8>>, wanted: usize) -> String {
        let mut all = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while all.len() < wanted {
            match tokio::time::timeout_at(deadline, receiver.recv()).await {
                Ok(Some(chunk)) => all.extend(chunk),
                _ => break,
            }
        }
        String::from_utf8(all).unwrap()
    }

    #[tokio::test]
    async fn every_message_is_its_own_datagram() {
        let collector = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = collector.local_addr().unwrap().port();
        let mut sender =
            SyslogSender::new(quick(Target::new("127.0.0.1", port, Transport::Udp))).unwrap();
        sender
            .send(&messages(&[
                "<13>1 - a - - - - one",
                "<13>1 - a - - - - two",
            ]))
            .await
            .unwrap();

        let mut datagram = [0u8; 2048];
        let mut received = Vec::new();
        for _ in 0..2 {
            let (length, _) =
                tokio::time::timeout(Duration::from_secs(2), collector.recv_from(&mut datagram))
                    .await
                    .expect("a datagram arrives")
                    .unwrap();
            received.push(String::from_utf8(datagram[..length].to_vec()).unwrap());
        }
        assert_eq!(
            received,
            vec!["<13>1 - a - - - - one", "<13>1 - a - - - - two"]
        );
    }

    #[tokio::test]
    async fn a_tcp_collector_gets_one_message_to_a_line() {
        let (port, mut seen) = tcp_collector().await;
        let mut sender =
            SyslogSender::new(quick(Target::new("127.0.0.1", port, Transport::Tcp))).unwrap();
        sender
            .send(&messages(&[
                "<13>1 - a - - - - one",
                "<13>1 - a - - - - two",
            ]))
            .await
            .unwrap();
        assert_eq!(
            collect(&mut seen, 44).await,
            "<13>1 - a - - - - one\n<13>1 - a - - - - two\n"
        );
        assert!(
            sender.is_connected(),
            "the connection is kept for the next batch"
        );
    }

    #[tokio::test]
    async fn a_line_break_inside_a_message_does_not_split_it_on_a_line_framed_stream() {
        let (port, mut seen) = tcp_collector().await;
        let mut sender =
            SyslogSender::new(quick(Target::new("127.0.0.1", port, Transport::Tcp))).unwrap();
        sender
            .send(&messages(&[
                "<13>1 - a - - - - one\r\n<0>1 - b - - - - forged",
            ]))
            .await
            .unwrap();
        assert_eq!(
            collect(&mut seen, 46).await,
            "<13>1 - a - - - - one  <0>1 - b - - - - forged\n"
        );
    }

    #[tokio::test]
    async fn our_own_listener_reads_both_framings_this_sender_writes() {
        for framing in [Framing::Newline, Framing::OctetCounted] {
            let (server, mut received) = crate::syslog::SyslogServer::start(
                "127.0.0.1:0".parse().unwrap(),
                Some("127.0.0.1:0".parse().unwrap()),
            )
            .await
            .unwrap();
            let port = server.tcp_addr().unwrap().port();
            let mut target = quick(Target::new("localhost", port, Transport::Tcp));
            target.framing = framing;
            let mut sender = SyslogSender::new(target).unwrap();
            let first = crate::syslog::format::Rfc5424 {
                facility: 23,
                severity: 3,
                hostname: Some("sw1"),
                msgid: Some("LINK-3-UPDOWN"),
                message: "Interface Gi0/1, changed state to down",
                ..Default::default()
            };
            let second = crate::syslog::format::Rfc5424 {
                facility: 23,
                severity: 5,
                hostname: Some("sw2"),
                message: "Configured from console",
                ..Default::default()
            };
            sender
                .send(&[first.encode(8192), second.encode(8192)])
                .await
                .unwrap();
            let mut hosts = Vec::new();
            for _ in 0..2 {
                let message = tokio::time::timeout(Duration::from_secs(2), received.recv())
                    .await
                    .unwrap_or_else(|_| panic!("a message arrives with {framing:?}"))
                    .unwrap();
                hosts.push((
                    message.hostname.unwrap(),
                    message.severity.unwrap(),
                    message.message,
                ));
            }
            assert_eq!(
                hosts,
                vec![
                    (
                        "sw1".to_owned(),
                        3,
                        "Interface Gi0/1, changed state to down".to_owned()
                    ),
                    ("sw2".to_owned(), 5, "Configured from console".to_owned()),
                ],
                "{framing:?}"
            );
        }
    }

    #[tokio::test]
    async fn over_tls_messages_are_octet_counted_and_the_certificate_is_checked() {
        let ca = TestCa::new();
        let identity = ca.issue(&["localhost"], valid_from(), valid_until());
        let (seen, mut received) = mpsc::unbounded_channel();
        let port = tls_server(server_config(identity), move |mut stream| {
            let seen = seen.clone();
            async move {
                let mut chunk = vec![0u8; 4096];
                while let Ok(count) = stream.read(&mut chunk).await {
                    if count == 0 || seen.send(chunk[..count].to_vec()).is_err() {
                        break;
                    }
                }
            }
        })
        .await;

        let mut target = quick(Target::new("localhost", port, Transport::Tls));
        assert_eq!(target.framing, Framing::OctetCounted);
        target.trust = Trust::Authorities(vec![ca.certificate.clone()]);
        let mut sender = SyslogSender::new(target).unwrap();
        sender
            .send(&messages(&[
                "<13>1 - a - - - - one",
                "<13>1 - a - - - - second",
            ]))
            .await
            .unwrap();
        assert_eq!(
            collect(&mut received, 51).await,
            "21 <13>1 - a - - - - one24 <13>1 - a - - - - second"
        );
    }

    #[tokio::test]
    async fn a_certificate_nobody_vouches_for_ends_the_attempt_before_any_log_text_is_sent() {
        let ca = TestCa::new();
        let identity = ca.issue(&["localhost"], valid_from(), valid_until());
        let (seen, mut received) = mpsc::unbounded_channel::<Vec<u8>>();
        let port = tls_server(server_config(identity), move |mut stream| {
            let seen = seen.clone();
            async move {
                let mut chunk = vec![0u8; 4096];
                while let Ok(count) = stream.read(&mut chunk).await {
                    if count == 0 || seen.send(chunk[..count].to_vec()).is_err() {
                        break;
                    }
                }
            }
        })
        .await;

        // The platform's authorities do not include a CA made a moment ago…
        let mut sender =
            SyslogSender::new(quick(Target::new("localhost", port, Transport::Tls))).unwrap();
        let failed = sender
            .send(&messages(&["<13>1 - a - - - - secret-ish"]))
            .await
            .unwrap_err();
        assert_eq!(failed.sent, 0);
        assert!(
            failed.error.to_string().contains("TLS handshake"),
            "{}",
            failed.error
        );
        // …and neither does another private CA.
        let other = TestCa::new();
        let mut target = quick(Target::new("localhost", port, Transport::Tls));
        target.trust = Trust::Authorities(vec![other.certificate.clone()]);
        let mut sender = SyslogSender::new(target).unwrap();
        let failed = sender
            .send(&messages(&["<13>1 - a - - - - secret-ish"]))
            .await
            .unwrap_err();
        // The other CA carries the same name, so the chain is found and its signature is
        // what does not hold: either way it is the certificate that is refused.
        let text = failed.error.to_string();
        assert!(
            text.contains("TLS handshake")
                && (text.contains("signature") || text.contains("authority")),
            "{text}"
        );
        assert!(!sender.is_connected());
        // …and the name on the certificate has to be the one that was dialled.
        let mut target = quick(Target::new("127.0.0.1", port, Transport::Tls));
        target.trust = Trust::Authorities(vec![ca.certificate.clone()]);
        let mut sender = SyslogSender::new(target).unwrap();
        let failed = sender
            .send(&messages(&["<13>1 - a - - - - secret-ish"]))
            .await
            .unwrap_err();
        assert!(
            failed
                .error
                .to_string()
                .contains("not valid for this host name"),
            "{}",
            failed.error
        );

        assert!(
            tokio::time::timeout(Duration::from_millis(200), received.recv())
                .await
                .is_err(),
            "no log text reached a collector whose certificate was refused"
        );
    }

    #[tokio::test]
    async fn a_port_that_does_not_speak_tls_is_said_to_be_one() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                drop(stream);
            }
        });
        let mut sender =
            SyslogSender::new(quick(Target::new("localhost", port, Transport::Tls))).unwrap();
        let failed = sender
            .send(&messages(&["<13>1 - a - - - - x"]))
            .await
            .unwrap_err();
        let text = failed.error.to_string();
        assert!(
            text.contains("TLS") || matches!(failed.error, ProbeError::Io(_)),
            "{text}"
        );
    }

    #[tokio::test]
    async fn a_collector_that_stops_reading_is_an_error_after_the_write_timeout_not_a_hang() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        // Accepted and then left alone: the connection is open and nothing is ever read.
        let held = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });

        let mut target = quick(Target::new("127.0.0.1", port, Transport::Tcp));
        target.write_timeout = Duration::from_millis(300);
        let mut sender = SyslogSender::new(target).unwrap();
        let line = vec![b'x'; 1024];
        let batch: Vec<Vec<u8>> = (0..1024).map(|_| line.clone()).collect();

        let started = Instant::now();
        let mut failure = None;
        // The operating system takes a few megabytes on the collector's behalf before a
        // write has to wait; 64 MB is far more than any socket buffer.
        for _ in 0..64 {
            if let Err(failed) = sender.send(&batch).await {
                failure = Some(failed);
                break;
            }
        }
        let failed = failure.expect("a collector that reads nothing cannot take 64 MB");
        assert!(
            failed.error.to_string().contains("stopped reading"),
            "{}",
            failed.error
        );
        assert_eq!(failed.sent, 0);
        assert!(
            !sender.is_connected(),
            "a stalled connection is not written to again"
        );
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "took {:?}",
            started.elapsed()
        );
        held.abort();
    }

    #[tokio::test]
    async fn a_connection_the_collector_closed_is_replaced_before_the_next_batch_is_written() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (seen, mut received) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut connection = 0;
            while let Ok((mut stream, _)) = listener.accept().await {
                connection += 1;
                let mut chunk = vec![0u8; 4096];
                // The first connection is closed after its first batch, as a collector
                // that restarts does; the second is read to its end.
                while let Ok(count) = stream.read(&mut chunk).await {
                    if count == 0 {
                        break;
                    }
                    let text = String::from_utf8(chunk[..count].to_vec()).unwrap();
                    let _ = seen.send((connection, text));
                    if connection == 1 {
                        break;
                    }
                }
            }
        });

        let mut sender =
            SyslogSender::new(quick(Target::new("127.0.0.1", port, Transport::Tcp))).unwrap();
        sender.send(&messages(&["first"])).await.unwrap();
        assert_eq!(received.recv().await.unwrap(), (1, "first\n".to_owned()));
        // Give the close time to cross the loopback.
        tokio::time::sleep(Duration::from_millis(100)).await;

        sender.send(&messages(&["second"])).await.unwrap();
        let arrived = tokio::time::timeout(Duration::from_secs(2), received.recv())
            .await
            .expect("the second batch arrives rather than vanishing into the closed connection")
            .unwrap();
        assert_eq!(arrived, (2, "second\n".to_owned()));
    }

    #[tokio::test]
    async fn nothing_listening_is_a_refusal_and_the_next_send_tries_again() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let mut sender =
            SyslogSender::new(quick(Target::new("127.0.0.1", port, Transport::Tcp))).unwrap();
        let failed = sender.send(&messages(&["x"])).await.unwrap_err();
        assert!(
            matches!(failed.error, ProbeError::Refused),
            "{}",
            failed.error
        );
        assert_eq!(failed.sent, 0);

        // The collector comes up on the same port.
        let listener = match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => listener,
            // Something else took the port in between; there is nothing to prove here.
            Err(_) => return,
        };
        let accepted = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut text = String::new();
            let mut chunk = [0u8; 64];
            let count = stream.read(&mut chunk).await.unwrap();
            text.push_str(std::str::from_utf8(&chunk[..count]).unwrap());
            text
        });
        sender.send(&messages(&["y"])).await.unwrap();
        assert_eq!(accepted.await.unwrap(), "y\n");
    }

    #[test]
    fn a_target_that_cannot_work_is_refused_before_anything_is_sent() {
        for (host, port, transport) in [
            ("", 514, Transport::Udp),
            ("two words", 514, Transport::Tcp),
            ("syslog://collector", 514, Transport::Tcp),
            ("collector", 0, Transport::Udp),
        ] {
            assert!(
                matches!(
                    SyslogSender::new(Target::new(host, port, transport)),
                    Err(ProbeError::Invalid(_))
                ),
                "{host:?}:{port}"
            );
        }
        assert!(
            SyslogSender::new(Target::new("collector.example.net", 6514, Transport::Tls)).is_ok()
        );
        assert!(SyslogSender::new(Target::new("2001:db8::1", 6514, Transport::Tls)).is_ok());
    }

    #[test]
    fn a_pem_that_holds_no_certificate_is_refused_with_the_reason() {
        let ca = TestCa::new();
        let pem = pem_of(&ca.certificate);
        assert_eq!(
            authorities_from_pem(&pem).unwrap(),
            vec![ca.certificate.clone()]
        );
        assert_eq!(
            authorities_from_pem(&format!("{pem}\n{pem}"))
                .unwrap()
                .len(),
            2
        );
        for nonsense in [
            "",
            "not a certificate",
            "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
        ] {
            assert!(
                matches!(authorities_from_pem(nonsense), Err(ProbeError::Invalid(_))),
                "{nonsense:?}"
            );
        }
    }

    /// DER as PEM, written out here because the crate has no need for an encoder elsewhere.
    fn pem_of(certificate: &CertificateDer<'_>) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut encoded = String::new();
        for chunk in certificate.as_ref().chunks(3) {
            let block = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let value = u32::from(block[0]) << 16 | u32::from(block[1]) << 8 | u32::from(block[2]);
            for index in 0..4 {
                if index <= chunk.len() {
                    encoded.push(ALPHABET[(value >> (18 - 6 * index) & 63) as usize] as char);
                } else {
                    encoded.push('=');
                }
            }
        }
        let lines: Vec<&str> = encoded
            .as_bytes()
            .chunks(64)
            .map(|line| std::str::from_utf8(line).unwrap())
            .collect();
        format!(
            "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
            lines.join("\n")
        )
    }
}

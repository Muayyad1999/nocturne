//! A small SMTP submission client.
//!
//! Just enough of RFC 5321 to hand one message to a relay: EHLO, STARTTLS
//! where the server offers it, AUTH PLAIN or LOGIN, MAIL/RCPT/DATA. It is
//! written straight on tokio and tokio-rustls so that e-mail notifications
//! need no C toolchain and no OpenSSL — the one condition the specification
//! put on shipping them at all.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

/// One message to send.
#[derive(Clone, Debug, Default)]
pub struct Mail {
    pub host: String,
    pub port: u16,
    /// `auto` (STARTTLS when offered), `required`, `never`.
    pub starttls: String,
    pub username: String,
    pub password: Option<String>,
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    pub body: String,
}

trait Duplex: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Duplex for T {}

struct Session {
    stream: BufReader<Box<dyn Duplex>>,
}

impl Session {
    async fn read_reply(&mut self) -> Result<(u16, String), String> {
        let mut text = String::new();
        loop {
            let mut line = String::new();
            let read = self
                .stream
                .read_line(&mut line)
                .await
                .map_err(|err| format!("the mail server closed the connection: {err}"))?;
            if read == 0 {
                return Err("the mail server closed the connection".into());
            }
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if trimmed.len() < 3 {
                return Err(format!("the mail server answered `{trimmed}`"));
            }
            let code: u16 = trimmed[..3]
                .parse()
                .map_err(|_| format!("the mail server answered `{trimmed}`"))?;
            text.push_str(trimmed);
            text.push('\n');
            // "250-" continues, "250 " ends.
            if trimmed.as_bytes().get(3) != Some(&b'-') {
                return Ok((code, text));
            }
        }
    }

    async fn write(&mut self, line: &str) -> Result<(), String> {
        self.stream
            .get_mut()
            .write_all(line.as_bytes())
            .await
            .map_err(|err| format!("could not write to the mail server: {err}"))?;
        self.stream
            .get_mut()
            .flush()
            .await
            .map_err(|err| format!("could not write to the mail server: {err}"))
    }

    async fn command(&mut self, line: &str, expect: u16) -> Result<String, String> {
        self.write(&format!("{line}\r\n")).await?;
        let (code, text) = self.read_reply().await?;
        if code / 100 != expect / 100 {
            let verb = line.split(' ').next().unwrap_or(line);
            return Err(format!(
                "the mail server refused {verb}: {}",
                text.trim().replace('\n', "; ")
            ));
        }
        Ok(text)
    }
}

/// Reject anything that would let a value break out of its header line.
fn header_safe(value: &str, what: &str) -> Result<String, String> {
    if value.contains(['\r', '\n']) || value.trim().is_empty() {
        return Err(format!("`{value}` is not a usable {what}"));
    }
    Ok(value.trim().to_string())
}

pub async fn send(mail: &Mail, timeout: Duration) -> Result<(), String> {
    tokio::time::timeout(timeout, exchange(mail))
        .await
        .map_err(|_| format!("the mail server {} did not answer in time", mail.host))?
}

async fn exchange(mail: &Mail) -> Result<(), String> {
    let from = header_safe(&mail.from, "sender address")?;
    if mail.to.is_empty() {
        return Err("the channel has nobody to send to".into());
    }
    let recipients: Vec<String> = mail
        .to
        .iter()
        .map(|address| header_safe(address, "recipient address"))
        .collect::<Result<_, _>>()?;
    let host = header_safe(&mail.host, "mail server name")?;

    let tcp = TcpStream::connect((host.as_str(), mail.port))
        .await
        .map_err(|err| {
            format!(
                "could not reach the mail server {host}:{}: {err}",
                mail.port
            )
        })?;
    let mut session = Session {
        stream: BufReader::new(Box::new(tcp) as Box<dyn Duplex>),
    };
    let (code, text) = session.read_reply().await?;
    if code != 220 {
        return Err(format!(
            "the mail server did not greet us: {}",
            text.trim().replace('\n', "; ")
        ));
    }
    let capabilities = session.command("EHLO nocturne", 250).await?;

    let wants_tls = match mail.starttls.as_str() {
        "never" => false,
        "required" => true,
        // auto
        _ => capabilities.to_ascii_uppercase().contains("STARTTLS"),
    };
    let capabilities = if wants_tls {
        if !capabilities.to_ascii_uppercase().contains("STARTTLS") && mail.starttls == "required" {
            return Err(format!(
                "the mail server {host} does not offer STARTTLS, and this channel requires it"
            ));
        }
        session.command("STARTTLS", 220).await?;
        upgrade(&mut session, &host).await?;
        session.command("EHLO nocturne", 250).await?
    } else {
        capabilities
    };

    if !mail.username.is_empty() {
        authenticate(&mut session, mail, &capabilities).await?;
    }

    session.command(&format!("MAIL FROM:<{from}>"), 250).await?;
    for address in &recipients {
        session
            .command(&format!("RCPT TO:<{address}>"), 250)
            .await?;
    }
    session.command("DATA", 354).await?;
    session.write(&message(mail, &from, &recipients)?).await?;
    session.command(".", 250).await?;
    // A server that will not say goodbye has still taken the message.
    let _ = session.command("QUIT", 221).await;
    Ok(())
}

async fn upgrade(session: &mut Session, host: &str) -> Result<(), String> {
    let provider = crate::crypto_provider();
    let verifier = rustls_platform_verifier::Verifier::new(provider.clone())
        .map_err(|err| format!("the system certificate store could not be read: {err}"))?;
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|err| format!("TLS could not be set up: {err}"))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|_| format!("`{host}` is not a name a certificate can be checked against"))?;
    // The server is forbidden from sending anything between its 220 and the
    // handshake, so nothing is lost by taking the stream out of its reader.
    let (placeholder, _other) = tokio::io::duplex(1);
    let plain = std::mem::replace(
        &mut session.stream,
        BufReader::new(Box::new(placeholder) as Box<dyn Duplex>),
    )
    .into_inner();
    let tls = TlsConnector::from(Arc::new(config))
        .connect(name, plain)
        .await
        .map_err(|err| format!("the TLS handshake with {host} failed: {err}"))?;
    session.stream = BufReader::new(Box::new(tls) as Box<dyn Duplex>);
    Ok(())
}

async fn authenticate(
    session: &mut Session,
    mail: &Mail,
    capabilities: &str,
) -> Result<(), String> {
    let password = mail.password.clone().unwrap_or_default();
    let mechanisms = capabilities.to_ascii_uppercase();
    if mechanisms.contains("AUTH") && mechanisms.contains("PLAIN") {
        let secret = BASE64.encode(format!("\0{}\0{password}", mail.username));
        session
            .command(&format!("AUTH PLAIN {secret}"), 235)
            .await?;
    } else if mechanisms.contains("AUTH") && mechanisms.contains("LOGIN") {
        session.command("AUTH LOGIN", 334).await?;
        session
            .command(&BASE64.encode(mail.username.as_bytes()), 334)
            .await?;
        session
            .command(&BASE64.encode(password.as_bytes()), 235)
            .await?;
    } else {
        return Err(format!(
            "the mail server {} offers no authentication this channel can use",
            mail.host
        ));
    }
    Ok(())
}

/// The message itself, with the dot-stuffing DATA requires.
fn message(mail: &Mail, from: &str, to: &[String]) -> Result<String, String> {
    let subject = header_safe(&mail.subject, "subject")?;
    let date = chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S +0000");
    let mut text = String::new();
    text.push_str(&format!("From: {from}\r\n"));
    text.push_str(&format!("To: {}\r\n", to.join(", ")));
    text.push_str(&format!("Subject: {subject}\r\n"));
    text.push_str(&format!("Date: {date}\r\n"));
    text.push_str("MIME-Version: 1.0\r\n");
    text.push_str("Content-Type: text/plain; charset=utf-8\r\n");
    text.push_str("X-Nocturne: alert\r\n\r\n");
    for line in mail.body.replace("\r\n", "\n").split('\n') {
        if let Some(rest) = line.strip_prefix('.') {
            text.push_str("..");
            text.push_str(rest);
        } else {
            text.push_str(line);
        }
        text.push_str("\r\n");
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail() -> Mail {
        Mail {
            host: "127.0.0.1".into(),
            port: 0,
            starttls: "never".into(),
            username: String::new(),
            password: None,
            from: "nocturne@example.net".into(),
            to: vec!["noc@example.net".into()],
            subject: "CORE1 is down".into(),
            body: "line one\n.hidden\nline three".into(),
        }
    }

    #[test]
    fn a_message_has_headers_crlf_line_ends_and_dot_stuffing() {
        let text = message(&mail(), "nocturne@example.net", &["noc@example.net".into()]).unwrap();
        assert!(text.contains("Subject: CORE1 is down\r\n"));
        assert!(text.contains("To: noc@example.net\r\n"));
        assert!(text.contains("\r\n\r\nline one\r\n"), "{text}");
        assert!(text.contains("\r\n..hidden\r\n"), "{text}");
    }

    #[test]
    fn header_injection_is_refused() {
        assert!(header_safe("a@b\r\nBcc: c@d", "sender address").is_err());
        assert!(header_safe("  ", "subject").is_err());
        assert_eq!(header_safe(" a@b ", "x").unwrap(), "a@b");
        let mut broken = mail();
        broken.subject = "down\r\nBcc: attacker@example".into();
        assert!(message(&broken, "a@b", &["c@d".into()]).is_err());
    }

    /// A relay that speaks just enough SMTP, so the conversation is checked
    /// end to end without a mail server.
    async fn relay(
        script_fail: bool,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut seen: Vec<String> = Vec::new();
            let mut reader = BufReader::new(stream);
            async fn say(reader: &mut BufReader<TcpStream>, text: &str) {
                reader.get_mut().write_all(text.as_bytes()).await.unwrap();
            }
            say(&mut reader, "220 relay ready\r\n").await;
            let mut in_data = false;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                    break;
                }
                let line = line.trim_end_matches(['\r', '\n']).to_string();
                seen.push(line.clone());
                if in_data {
                    if line == "." {
                        in_data = false;
                        say(&mut reader, "250 queued\r\n").await;
                    }
                    continue;
                }
                let reply = match line.split(' ').next().unwrap_or("") {
                    "EHLO" => "250-relay\r\n250-AUTH PLAIN LOGIN\r\n250 SIZE 10240000\r\n",
                    "AUTH" => "235 authenticated\r\n",
                    "MAIL" if script_fail => "550 no relaying\r\n",
                    "MAIL" | "RCPT" => "250 ok\r\n",
                    "DATA" => {
                        in_data = true;
                        "354 go ahead\r\n"
                    }
                    "QUIT" => "221 bye\r\n",
                    _ => "250 ok\r\n",
                };
                say(&mut reader, reply).await;
                if line == "QUIT" {
                    break;
                }
            }
            seen
        });
        (address, handle)
    }

    #[tokio::test]
    async fn a_message_goes_through_a_relay_with_plain_authentication() {
        let (address, handle) = relay(false).await;
        let mut mail = mail();
        mail.host = address.ip().to_string();
        mail.port = address.port();
        mail.username = "noc".into();
        mail.password = Some("pw".into());
        send(&mail, Duration::from_secs(5)).await.unwrap();
        let seen = handle.await.unwrap();
        assert!(seen.iter().any(|line| line == "EHLO nocturne"));
        assert!(
            seen.iter().any(|line| line.starts_with("AUTH PLAIN ")),
            "{seen:?}"
        );
        assert!(
            seen.contains(&"MAIL FROM:<nocturne@example.net>".to_string()),
            "{seen:?}"
        );
        assert!(seen.contains(&"RCPT TO:<noc@example.net>".to_string()));
        assert!(seen.contains(&"Subject: CORE1 is down".to_string()));
        // The password never appears in the clear.
        assert!(!seen.iter().any(|line| line.contains("pw")));
    }

    #[tokio::test]
    async fn a_refusal_comes_back_as_an_error_naming_the_step() {
        let (address, _handle) = relay(true).await;
        let mut mail = mail();
        mail.host = address.ip().to_string();
        mail.port = address.port();
        let error = send(&mail, Duration::from_secs(5)).await.unwrap_err();
        assert!(error.contains("MAIL"), "{error}");
        assert!(error.contains("no relaying"), "{error}");
    }

    #[tokio::test]
    async fn requiring_starttls_from_a_server_that_has_none_says_so() {
        let (address, _handle) = relay(false).await;
        let mut mail = mail();
        mail.host = address.ip().to_string();
        mail.port = address.port();
        mail.starttls = "required".into();
        let error = send(&mail, Duration::from_secs(5)).await.unwrap_err();
        assert!(error.contains("STARTTLS"), "{error}");
    }
}

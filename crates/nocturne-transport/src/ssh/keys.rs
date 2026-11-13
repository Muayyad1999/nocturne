//! Host-key verification and key handling.

use std::collections::HashMap;
use std::sync::Mutex;

use russh::keys::ssh_key::private::{KeypairData, RsaKeypair};
use russh::keys::ssh_key::{self, LineEnding};
use russh::keys::{Algorithm, HashAlg, PrivateKey, PublicKey};

use crate::error::TransportError;
use crate::net::host_port;

/// The host key a server presented during the handshake.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresentedKey {
    /// Key type, e.g. `ssh-ed25519`, `ssh-rsa`, `ecdsa-sha2-nistp256`.
    pub algorithm: String,
    /// `SHA256:…`, as printed by `ssh-keygen -l`.
    pub fingerprint: String,
    /// `<algorithm> <base64>`, as found in `known_hosts`.
    pub openssh: String,
}

impl PresentedKey {
    pub(crate) fn from_public(key: &PublicKey) -> Self {
        let openssh = key
            .to_openssh()
            .map(|line| normalize(&line))
            .unwrap_or_default();
        Self {
            algorithm: key.algorithm().as_str().to_owned(),
            fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
            openssh,
        }
    }
}

/// Decides whether a server's host key is trusted.
#[async_trait::async_trait]
pub trait HostKeyVerifier: Send + Sync {
    /// Return Ok to continue, Err(reason) to abort the handshake.
    ///
    /// The handshake timeout is suspended while this runs, so an
    /// implementation may ask the user.
    async fn verify(&self, host: &str, port: u16, key: &PresentedKey) -> Result<(), String>;
}

/// Accepts everything (tests, lab).
#[derive(Clone, Copy, Debug, Default)]
pub struct AcceptAnyHostKey;

#[async_trait::async_trait]
impl HostKeyVerifier for AcceptAnyHostKey {
    async fn verify(&self, _host: &str, _port: u16, _key: &PresentedKey) -> Result<(), String> {
        Ok(())
    }
}

/// Trust-on-first-use against an in-memory map; rejects changed keys.
///
/// The first key seen for a `host:port` is remembered; afterwards only a
/// remembered key is accepted. Entries can be pre-loaded and exported, so an
/// application can persist them.
#[derive(Debug, Default)]
pub struct MemoryKnownHosts {
    hosts: Mutex<HashMap<(String, u16), Vec<String>>>,
}

impl MemoryKnownHosts {
    pub fn new() -> Self {
        Self::default()
    }

    /// Trust `openssh_key` (`<algorithm> <base64> [comment]`) for `host:port`,
    /// in addition to any key already known for it.
    pub fn insert(&self, host: &str, port: u16, openssh_key: &str) {
        let key = normalize(openssh_key);
        let mut hosts = self.lock();
        let known = hosts.entry(entry_key(host, port)).or_default();
        if !known.contains(&key) {
            known.push(key);
        }
    }

    /// Forget everything known about `host:port`; `true` if there was something.
    pub fn remove(&self, host: &str, port: u16) -> bool {
        self.lock().remove(&entry_key(host, port)).is_some()
    }

    /// The keys trusted for `host:port`.
    pub fn get(&self, host: &str, port: u16) -> Vec<String> {
        self.lock()
            .get(&entry_key(host, port))
            .cloned()
            .unwrap_or_default()
    }

    /// Every `(host, port, key)` entry, for persisting.
    pub fn entries(&self) -> Vec<(String, u16, String)> {
        let mut entries: Vec<_> = self
            .lock()
            .iter()
            .flat_map(|((host, port), keys)| {
                keys.iter()
                    .map(move |key| (host.clone(), *port, key.clone()))
            })
            .collect();
        entries.sort();
        entries
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<(String, u16), Vec<String>>> {
        // The map stays consistent even if a holder panicked.
        self.hosts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[async_trait::async_trait]
impl HostKeyVerifier for MemoryKnownHosts {
    async fn verify(&self, host: &str, port: u16, key: &PresentedKey) -> Result<(), String> {
        let presented = normalize(&key.openssh);
        let mut hosts = self.lock();
        let known = hosts.entry(entry_key(host, port)).or_default();
        if known.is_empty() {
            known.push(presented);
            return Ok(());
        }
        if known.contains(&presented) {
            return Ok(());
        }
        let expected: Vec<String> = known.iter().map(|key| describe(key)).collect();
        Err(format!(
            "the host key of {} has changed: expected {}, but the server presented {} {} \
             (possible man-in-the-middle attack, or the host was reinstalled)",
            host_port(host, port),
            expected.join(" or "),
            key.algorithm,
            key.fingerprint,
        ))
    }
}

fn entry_key(host: &str, port: u16) -> (String, u16) {
    (host.trim().to_ascii_lowercase(), port)
}

/// `<algorithm> <base64>` without comment or stray whitespace.
fn normalize(openssh_key: &str) -> String {
    openssh_key
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ")
}

/// `<algorithm> <fingerprint>` for a stored key line.
fn describe(openssh_key: &str) -> String {
    match PublicKey::from_openssh(openssh_key) {
        Ok(key) => format!(
            "{} {}",
            key.algorithm().as_str(),
            key.fingerprint(HashAlg::Sha256)
        ),
        Err(_) => openssh_key.to_owned(),
    }
}

/// Which kind of key pair [`generate_keypair`] creates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyKind {
    Ed25519,
    Rsa3072,
    Rsa4096,
}

/// Generate an Ed25519 or RSA key pair: (private key in OpenSSH format, public key line).
///
/// RSA generation is CPU-bound and can take seconds; call it from
/// `spawn_blocking` in async code.
pub fn generate_keypair(kind: KeyKind, comment: &str) -> Result<(String, String), TransportError> {
    let failed =
        |error: ssh_key::Error| TransportError::Protocol(format!("key generation failed: {error}"));
    let mut rng = russh::keys::key::safe_rng();
    let mut key = match kind {
        KeyKind::Ed25519 => PrivateKey::random(&mut rng, Algorithm::Ed25519).map_err(failed)?,
        KeyKind::Rsa3072 | KeyKind::Rsa4096 => {
            let bits = if kind == KeyKind::Rsa3072 { 3072 } else { 4096 };
            let pair = RsaKeypair::random(&mut rng, bits).map_err(failed)?;
            PrivateKey::new(KeypairData::Rsa(pair), "").map_err(failed)?
        }
    };
    key.set_comment(comment);
    let private = key.to_openssh(LineEnding::LF).map_err(failed)?;
    let public = key.public_key().to_openssh().map_err(failed)?;
    Ok((private.to_string(), public))
}

/// Parses private key text (OpenSSH, PKCS#1/PKCS#8 PEM or PuTTY format).
pub(crate) fn decode_private_key(
    text: &str,
    passphrase: Option<&str>,
) -> Result<PrivateKey, String> {
    let passphrase = passphrase.filter(|p| !p.is_empty());
    russh::keys::decode_secret_key(text, passphrase).map_err(|error| match error {
        russh::keys::Error::KeyIsEncrypted => {
            "the key is encrypted and no passphrase was given".to_owned()
        }
        russh::keys::Error::SshKey(ssh_key::Error::Crypto) if passphrase.is_some() => {
            "wrong passphrase".to_owned()
        }
        other => other.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn presented(private_key: &str) -> PresentedKey {
        let key = decode_private_key(private_key, None).unwrap();
        PresentedKey::from_public(key.public_key())
    }

    #[test]
    fn ed25519_keys_are_generated_in_openssh_format() {
        let (private, public) = generate_keypair(KeyKind::Ed25519, "nocturne@test").unwrap();
        assert!(private.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----\n"));
        assert!(
            private
                .trim_end()
                .ends_with("-----END OPENSSH PRIVATE KEY-----")
        );
        assert!(public.starts_with("ssh-ed25519 AAAA"));
        assert!(public.ends_with(" nocturne@test"));

        let key = decode_private_key(&private, None).unwrap();
        assert_eq!(
            normalize(&key.public_key().to_openssh().unwrap()),
            normalize(&public)
        );
    }

    #[test]
    fn rsa_keys_have_the_requested_size() {
        let (private, public) = generate_keypair(KeyKind::Rsa3072, "").unwrap();
        assert!(public.starts_with("ssh-rsa AAAA"));
        let key = decode_private_key(&private, None).unwrap();
        let KeypairData::Rsa(pair) = key.key_data() else {
            panic!("not an RSA key");
        };
        let modulus = pair.public().n().as_positive_bytes().unwrap();
        assert_eq!(modulus.len() * 8, 3072);
    }

    #[test]
    fn presented_key_has_fingerprint_and_known_hosts_form() {
        let (private, public) = generate_keypair(KeyKind::Ed25519, "ignored comment").unwrap();
        let key = presented(&private);
        assert_eq!(key.algorithm, "ssh-ed25519");
        assert!(key.fingerprint.starts_with("SHA256:"));
        assert_eq!(key.openssh, normalize(&public));
        assert_eq!(key.openssh.split(' ').count(), 2);
    }

    #[test]
    fn bad_keys_give_readable_errors() {
        assert!(decode_private_key("not a key", None).is_err());
        let (private, _) = generate_keypair(KeyKind::Ed25519, "").unwrap();
        let key = decode_private_key(&private, None).unwrap();
        let encrypted = key
            .encrypt(&mut russh::keys::key::safe_rng(), "pass")
            .unwrap()
            .to_openssh(LineEnding::LF)
            .unwrap();
        let error = decode_private_key(&encrypted, None).unwrap_err();
        assert!(error.contains("encrypted"), "{error}");
        let error = decode_private_key(&encrypted, Some("wrong")).unwrap_err();
        assert!(error.contains("wrong passphrase"), "{error}");
        assert!(decode_private_key(&encrypted, Some("pass")).is_ok());
    }

    #[tokio::test]
    async fn accept_any_accepts() {
        let (private, _) = generate_keypair(KeyKind::Ed25519, "").unwrap();
        assert!(
            AcceptAnyHostKey
                .verify("h", 22, &presented(&private))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn tofu_remembers_the_first_key_and_rejects_a_changed_one() {
        let (first, _) = generate_keypair(KeyKind::Ed25519, "").unwrap();
        let (second, _) = generate_keypair(KeyKind::Ed25519, "").unwrap();
        let (first, second) = (presented(&first), presented(&second));
        let known = MemoryKnownHosts::new();

        assert!(known.verify("Router1", 22, &first).await.is_ok());
        assert!(known.verify("router1", 22, &first).await.is_ok());
        assert_eq!(known.get("router1", 22), vec![first.openssh.clone()]);

        let reason = known.verify("router1", 22, &second).await.unwrap_err();
        assert!(reason.contains("has changed"), "{reason}");
        assert!(reason.contains(&first.fingerprint), "{reason}");
        assert!(reason.contains(&second.fingerprint), "{reason}");

        // Another port is another host.
        assert!(known.verify("router1", 2222, &second).await.is_ok());
        assert_eq!(known.entries().len(), 2);
    }

    #[tokio::test]
    async fn known_hosts_can_be_preloaded_and_edited() {
        let (first, public) = generate_keypair(KeyKind::Ed25519, "a comment").unwrap();
        let (second, _) = generate_keypair(KeyKind::Ed25519, "").unwrap();
        let (first, second) = (presented(&first), presented(&second));
        let known = MemoryKnownHosts::new();

        known.insert("sw1", 22, &public);
        known.insert("sw1", 22, &public);
        assert_eq!(known.get("sw1", 22).len(), 1);
        assert!(known.verify("sw1", 22, &first).await.is_ok());
        assert!(known.verify("sw1", 22, &second).await.is_err());

        // A second trusted key (e.g. another key type of the same host).
        known.insert("sw1", 22, &second.openssh);
        assert!(known.verify("sw1", 22, &second).await.is_ok());

        assert!(known.remove("sw1", 22));
        assert!(!known.remove("sw1", 22));
        assert!(known.verify("sw1", 22, &second).await.is_ok());
    }
}

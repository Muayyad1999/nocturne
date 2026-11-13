//! SSH client: sessions, shells, exec, SFTP, jump hosts.

use std::fmt;

use serde::{Deserialize, Serialize};

mod algorithms;
mod auth;
mod client;
mod jump;
mod keys;
mod sftp;

pub use client::{ExecOutput, SshSession, connect_ssh};
pub use keys::{
    AcceptAnyHostKey, HostKeyVerifier, KeyKind, MemoryKnownHosts, PresentedKey, generate_keypair,
};
pub use sftp::{Sftp, SftpEntry};

pub(crate) use algorithms::{CryptoPolicy, preferred as preferred_algorithms};
pub(crate) use keys::decode_private_key;

/// Most hops a `ProxyJump` chain may have.
pub(crate) const MAX_JUMP_HOSTS: usize = 5;

/// How to reach and log in to an SSH server.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SshParams {
    pub host: String,
    pub port: u16,
    pub username: String,
    /// Authentication methods, tried in order.
    pub auth: Vec<SshAuth>,
    /// `ProxyJump` chain, first hop first (at most five).
    pub jump: Vec<SshHop>,
    /// Terminal type requested for shells.
    pub term: String,
    pub cols: u16,
    pub rows: u16,
    /// Bounds the TCP connection and, separately, the SSH handshake.
    pub connect_timeout_ms: u64,
    /// Seconds of silence before a keep-alive is sent; `0` turns them off.
    pub keepalive_s: u64,
    pub legacy: LegacyCrypto,
}

impl Default for SshParams {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 22,
            username: String::new(),
            auth: Vec::new(),
            jump: Vec::new(),
            term: "xterm-256color".to_owned(),
            cols: 120,
            rows: 40,
            connect_timeout_ms: 10_000,
            keepalive_s: 30,
            legacy: LegacyCrypto::Auto,
        }
    }
}

/// One jump host of a `ProxyJump` chain.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SshHop {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: Vec<SshAuth>,
}

impl Default for SshHop {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 22,
            username: String::new(),
            auth: Vec::new(),
        }
    }
}

/// One way of proving who we are.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SshAuth {
    /// Also used to answer keyboard-interactive prompts when the server does
    /// not offer plain password authentication.
    Password { password: String },
    /// OpenSSH / PEM private key text (RSA, ECDSA, Ed25519), optionally encrypted.
    PrivateKey {
        key: String,
        passphrase: Option<String>,
    },
    KeyFile {
        path: String,
        passphrase: Option<String>,
    },
    /// Answers every keyboard-interactive prompt with `response`.
    KeyboardInteractive { response: String },
    /// Keys offered by the running ssh-agent (`SSH_AUTH_SOCK` / Pageant pipe).
    Agent,
    None,
}

/// Secrets never reach logs: `Debug` shows the shape of the method only.
impl fmt::Debug for SshAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const REDACTED: &str = "<redacted>";
        let passphrase = |p: &Option<String>| p.as_ref().map(|_| REDACTED);
        match self {
            Self::Password { .. } => f
                .debug_struct("Password")
                .field("password", &REDACTED)
                .finish(),
            Self::PrivateKey { passphrase: p, .. } => f
                .debug_struct("PrivateKey")
                .field("key", &REDACTED)
                .field("passphrase", &passphrase(p))
                .finish(),
            Self::KeyFile {
                path,
                passphrase: p,
            } => f
                .debug_struct("KeyFile")
                .field("path", path)
                .field("passphrase", &passphrase(p))
                .finish(),
            Self::KeyboardInteractive { .. } => f
                .debug_struct("KeyboardInteractive")
                .field("response", &REDACTED)
                .finish(),
            Self::Agent => f.write_str("Agent"),
            Self::None => f.write_str("None"),
        }
    }
}

/// Whether algorithms that are obsolete but still common on network gear may
/// be negotiated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyCrypto {
    /// Modern algorithms first; if negotiation fails, retry once with the
    /// legacy set and flag the session.
    #[default]
    Auto,
    Never,
    Always,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_default_and_partial_json() {
        let params: SshParams = serde_json::from_str(
            r#"{"host":"10.0.0.1","username":"admin","auth":[{"type":"password","password":"pw"},{"type":"agent"},{"type":"key_file","path":"/k"}],"jump":[{"host":"bastion"}]}"#,
        )
        .unwrap();
        assert_eq!(params.port, 22);
        assert_eq!(params.term, "xterm-256color");
        assert_eq!((params.cols, params.rows), (120, 40));
        assert_eq!(params.connect_timeout_ms, 10_000);
        assert_eq!(params.keepalive_s, 30);
        assert_eq!(params.legacy, LegacyCrypto::Auto);
        assert_eq!(params.jump[0].port, 22);
        assert!(matches!(params.auth[0], SshAuth::Password { .. }));
        assert!(matches!(params.auth[1], SshAuth::Agent));
        assert!(matches!(&params.auth[2], SshAuth::KeyFile { passphrase: None, .. }));

        let legacy: LegacyCrypto = serde_json::from_str(r#""always""#).unwrap();
        assert_eq!(legacy, LegacyCrypto::Always);
        let json = serde_json::to_value(SshAuth::KeyboardInteractive {
            response: "x".into(),
        })
        .unwrap();
        assert_eq!(json["type"], "keyboard_interactive");
    }

    #[test]
    fn debug_never_prints_secrets() {
        let params = SshParams {
            auth: vec![
                SshAuth::Password {
                    password: "hunter2".into(),
                },
                SshAuth::PrivateKey {
                    key: "-----BEGIN OPENSSH PRIVATE KEY-----".into(),
                    passphrase: Some("s3cret".into()),
                },
                SshAuth::KeyboardInteractive {
                    response: "otp-123".into(),
                },
                SshAuth::KeyFile {
                    path: "/home/me/.ssh/id".into(),
                    passphrase: Some("s3cret".into()),
                },
            ],
            ..SshParams::default()
        };
        let text = format!("{params:?}");
        for secret in ["hunter2", "BEGIN OPENSSH", "s3cret", "otp-123"] {
            assert!(!text.contains(secret), "{secret} leaked into {text}");
        }
        assert!(text.contains("/home/me/.ssh/id"));
    }
}

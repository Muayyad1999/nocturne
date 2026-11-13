//! Algorithm preference sets and the legacy-crypto policy.
//!
//! Two sets are defined:
//!
//! * the **modern** set follows current OpenSSH defaults (minus what russh
//!   does not implement);
//! * the **legacy** additions are algorithms that are obsolete but are all an
//!   old switch or router may speak. They are appended *after* the modern
//!   ones, so a legacy-enabled client still picks the best the server has.
//!
//! What this russh build (0.65, features `ring`, `rsa`, `dsa`, `des`) cannot
//! negotiate at all, however the policy is set: `hmac-md5`, `hmac-md5-96`,
//! `hmac-sha1-96`, `hmac-ripemd160`, `umac-*`, `arcfour*`, `blowfish-cbc`,
//! `cast128-cbc`, `sntrup761x25519-sha512`, group-exchange groups smaller
//! than 2048 bits, and SSH protocol 1.

use std::borrow::Cow;
use std::time::Duration;

use russh::keys::{Algorithm, EcdsaCurve, HashAlg};
use russh::{AlgorithmKind, Limits, Preferred, cipher, client, compression, kex, mac};

/// Which preference set a connection attempt uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CryptoPolicy {
    Modern,
    /// Modern first, then the legacy additions.
    Legacy,
}

const MODERN_KEX: &[kex::Name] = &[
    kex::MLKEM768X25519_SHA256,
    kex::CURVE25519,
    kex::CURVE25519_PRE_RFC_8731,
    kex::ECDH_SHA2_NISTP256,
    kex::ECDH_SHA2_NISTP384,
    kex::ECDH_SHA2_NISTP521,
    kex::DH_GEX_SHA256,
    kex::DH_G16_SHA512,
    kex::DH_G18_SHA512,
    kex::DH_G17_SHA512,
    kex::DH_G15_SHA512,
    kex::DH_G14_SHA256,
];

/// Fixed 2048-bit group first: it always completes, whereas group exchange
/// fails against devices that only have groups under 2048 bits.
const LEGACY_KEX: &[kex::Name] = &[kex::DH_G14_SHA1, kex::DH_GEX_SHA1, kex::DH_G1_SHA1];

const MODERN_CIPHERS: &[cipher::Name] = &[
    cipher::CHACHA20_POLY1305,
    cipher::AES_256_GCM,
    cipher::AES_128_GCM,
    cipher::AES_256_CTR,
    cipher::AES_192_CTR,
    cipher::AES_128_CTR,
];

const LEGACY_CIPHERS: &[cipher::Name] = &[
    cipher::AES_256_CBC,
    cipher::AES_192_CBC,
    cipher::AES_128_CBC,
    cipher::TRIPLE_DES_CBC,
];

const MODERN_MACS: &[mac::Name] = &[
    mac::HMAC_SHA256_ETM,
    mac::HMAC_SHA512_ETM,
    mac::HMAC_SHA256,
    mac::HMAC_SHA512,
];

const LEGACY_MACS: &[mac::Name] = &[mac::HMAC_SHA1_ETM, mac::HMAC_SHA1];

const COMPRESSION: &[compression::Name] = &[
    compression::NONE,
    compression::ZLIB_LEGACY,
    compression::ZLIB,
];

fn modern_host_keys() -> Vec<Algorithm> {
    vec![
        Algorithm::Ed25519,
        Algorithm::Ecdsa {
            curve: EcdsaCurve::NistP256,
        },
        Algorithm::Ecdsa {
            curve: EcdsaCurve::NistP384,
        },
        Algorithm::Ecdsa {
            curve: EcdsaCurve::NistP521,
        },
        Algorithm::Rsa {
            hash: Some(HashAlg::Sha512),
        },
        Algorithm::Rsa {
            hash: Some(HashAlg::Sha256),
        },
    ]
}

/// `ssh-rsa` (RSA with SHA-1) and `ssh-dss`.
fn legacy_host_keys() -> Vec<Algorithm> {
    vec![Algorithm::Rsa { hash: None }, Algorithm::Dsa]
}

/// Which end of the connection a preference list is for (the protocol
/// extension markers in the kex list differ).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    Client,
    Server,
}

/// The algorithm preferences for one connection attempt.
pub(crate) fn preferred(policy: CryptoPolicy, role: Role) -> Preferred {
    let legacy = policy == CryptoPolicy::Legacy;
    let with = |modern: &[_], extra: &[_]| -> Vec<_> {
        let mut list = modern.to_vec();
        if legacy {
            list.extend_from_slice(extra);
        }
        list
    };

    let mut kex = with(MODERN_KEX, LEGACY_KEX);
    kex.extend_from_slice(match role {
        Role::Client => &[
            kex::EXTENSION_SUPPORT_AS_CLIENT,
            kex::EXTENSION_OPENSSH_STRICT_KEX_AS_CLIENT,
        ],
        Role::Server => &[
            kex::EXTENSION_SUPPORT_AS_SERVER,
            kex::EXTENSION_OPENSSH_STRICT_KEX_AS_SERVER,
        ],
    });
    let mut key = modern_host_keys();
    if legacy {
        key.extend(legacy_host_keys());
    }

    Preferred {
        kex: Cow::Owned(kex),
        key: Cow::Owned(key),
        host_key_certificates: Cow::Borrowed(&[]),
        cipher: Cow::Owned(with(MODERN_CIPHERS, LEGACY_CIPHERS)),
        mac: Cow::Owned(with(MODERN_MACS, LEGACY_MACS)),
        compression: Cow::Borrowed(COMPRESSION),
    }
}

/// russh client configuration for one connection attempt.
pub(crate) fn client_config(policy: CryptoPolicy, keepalive: Option<Duration>) -> client::Config {
    let (gex, limits) = match policy {
        // OpenSSH's own floor is 2048 bits.
        CryptoPolicy::Modern => (client::GexParams::new(2048, 3072, 8192), Limits::default()),
        // Old CPUs take ages over large groups, and old firmware often
        // mishandles a client-initiated re-key: keep the group small and only
        // re-key on volume (like OpenSSH, which has no time limit by default).
        CryptoPolicy::Legacy => (
            client::GexParams::new(2048, 2048, 8192),
            Limits {
                rekey_time_limit: Duration::from_secs(u32::MAX.into()),
                ..Limits::default()
            },
        ),
    };
    client::Config {
        client_id: russh::SshId::Standard(Cow::Borrowed(concat!(
            "SSH-2.0-Nocturne_",
            env!("CARGO_PKG_VERSION")
        ))),
        preferred: preferred(policy, Role::Client),
        gex: gex.unwrap_or_default(),
        limits,
        keepalive_interval: keepalive,
        keepalive_max: 3,
        inactivity_timeout: None,
        // The socket is already configured by `net::connect`.
        nodelay: true,
        ..client::Config::default()
    }
}

/// The algorithms a key exchange settled on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Negotiated {
    pub kex: String,
    pub host_key: String,
    pub cipher: String,
    pub client_mac: String,
    pub server_mac: String,
}

impl Negotiated {
    pub fn from_names(names: &russh::Names) -> Self {
        Self {
            kex: names.kex.as_ref().to_owned(),
            host_key: names.key.to_string(),
            cipher: names.cipher.as_ref().to_owned(),
            client_mac: names.client_mac.as_ref().to_owned(),
            server_mac: names.server_mac.as_ref().to_owned(),
        }
    }

    /// AEAD ciphers authenticate packets themselves; the negotiated MAC is unused.
    fn has_implicit_mac(&self) -> bool {
        [
            cipher::CHACHA20_POLY1305,
            cipher::AES_256_GCM,
            cipher::AES_128_GCM,
        ]
        .iter()
        .any(|aead| aead.as_ref() == self.cipher)
    }

    /// The MAC in use, `None` for AEAD ciphers.
    pub fn mac(&self) -> Option<String> {
        if self.has_implicit_mac() {
            None
        } else if self.client_mac == self.server_mac {
            Some(self.client_mac.clone())
        } else {
            Some(format!("{} / {}", self.client_mac, self.server_mac))
        }
    }

    /// Whether any algorithm in use comes from the legacy additions.
    pub fn uses_legacy(&self) -> bool {
        let legacy_mac = |name: &str| LEGACY_MACS.iter().any(|m| m.as_ref() == name);
        LEGACY_KEX.iter().any(|k| k.as_ref() == self.kex)
            || legacy_host_keys()
                .iter()
                .any(|k| k.as_str() == self.host_key)
            || LEGACY_CIPHERS.iter().any(|c| c.as_ref() == self.cipher)
            || (!self.has_implicit_mac()
                && (legacy_mac(&self.client_mac) || legacy_mac(&self.server_mac)))
    }
}

/// Every name the given policy can negotiate for `kind`.
fn supported(kind: &AlgorithmKind, policy: CryptoPolicy) -> Vec<String> {
    let preferred = preferred(policy, Role::Client);
    match kind {
        AlgorithmKind::Kex => names(&preferred.kex),
        AlgorithmKind::Key => preferred.key.iter().map(ToString::to_string).collect(),
        AlgorithmKind::Cipher => names(&preferred.cipher),
        AlgorithmKind::Mac => names(&preferred.mac),
        AlgorithmKind::Compression => names(&preferred.compression),
    }
}

fn names<T: AsRef<str>>(list: &[T]) -> Vec<String> {
    list.iter().map(|name| name.as_ref().to_owned()).collect()
}

/// Kex-list entries that signal protocol extensions rather than algorithms.
fn is_extension_marker(name: &str) -> bool {
    name.starts_with("ext-info-") || name.starts_with("kex-strict-")
}

/// Explains a failed negotiation: what the server offered and what the user
/// can do about it.
pub(crate) fn describe_no_common(
    kind: &AlgorithmKind,
    theirs: &[String],
    policy: CryptoPolicy,
) -> String {
    let label = match kind {
        AlgorithmKind::Kex => "key exchange",
        AlgorithmKind::Key => "host key",
        AlgorithmKind::Cipher => "cipher",
        AlgorithmKind::Mac => "MAC",
        AlgorithmKind::Compression => "compression",
    };
    let offered: Vec<&str> = theirs
        .iter()
        .map(String::as_str)
        .filter(|name| !is_extension_marker(name))
        .collect();
    let offered_text = if offered.is_empty() {
        "nothing".to_owned()
    } else {
        offered.join(", ")
    };

    let with_legacy = supported(kind, CryptoPolicy::Legacy);
    let usable_with_legacy: Vec<&str> = offered
        .iter()
        .copied()
        .filter(|name| with_legacy.iter().any(|ours| ours == name))
        .collect();

    let advice = match policy {
        CryptoPolicy::Modern if !usable_with_legacy.is_empty() => format!(
            "{} only available with legacy crypto: set `legacy` to `auto` or `always` to connect",
            if usable_with_legacy.len() == 1 {
                format!("{} is a legacy algorithm", usable_with_legacy[0])
            } else {
                format!("{} are legacy algorithms", usable_with_legacy.join(", "))
            }
        ),
        _ => "none of them is supported by this client, even with legacy crypto enabled"
            .to_owned(),
    };
    format!("no common {label} algorithm; the server offers: {offered_text}; {advice}")
}

/// The message for a server that only takes SHA-1 RSA signatures from users
/// while legacy crypto is disabled.
pub(crate) fn describe_sha1_signature_refused() -> String {
    "no common public key signature algorithm; the server offers: ssh-rsa; ssh-rsa (RSA with \
     SHA-1) is a legacy algorithm only available with legacy crypto: set `legacy` to `auto` or \
     `always` to connect"
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn negotiated(kex: &str, host_key: &str, cipher: &str, mac: &str) -> Negotiated {
        Negotiated {
            kex: kex.into(),
            host_key: host_key.into(),
            cipher: cipher.into(),
            client_mac: mac.into(),
            server_mac: mac.into(),
        }
    }

    #[test]
    fn modern_set_has_no_legacy_algorithms() {
        let modern = preferred(CryptoPolicy::Modern, Role::Client);
        for name in LEGACY_KEX {
            assert!(!modern.kex.contains(name));
        }
        for name in LEGACY_CIPHERS {
            assert!(!modern.cipher.contains(name));
        }
        for name in LEGACY_MACS {
            assert!(!modern.mac.contains(name));
        }
        for key in legacy_host_keys() {
            assert!(!modern.key.contains(&key));
        }
        assert!(modern.kex.contains(&kex::EXTENSION_SUPPORT_AS_CLIENT));
        assert!(
            modern
                .kex
                .contains(&kex::EXTENSION_OPENSSH_STRICT_KEX_AS_CLIENT)
        );
    }

    #[test]
    fn legacy_set_keeps_modern_algorithms_first() {
        let modern = preferred(CryptoPolicy::Modern, Role::Client);
        let legacy = preferred(CryptoPolicy::Legacy, Role::Client);
        assert_eq!(legacy.cipher[..MODERN_CIPHERS.len()], modern.cipher[..]);
        assert_eq!(legacy.mac[..MODERN_MACS.len()], modern.mac[..]);
        assert_eq!(legacy.kex[..MODERN_KEX.len()], modern.kex[..MODERN_KEX.len()]);
        let names: Vec<&str> = legacy.kex.iter().map(AsRef::as_ref).collect();
        for expected in [
            "diffie-hellman-group1-sha1",
            "diffie-hellman-group14-sha1",
            "diffie-hellman-group-exchange-sha1",
        ] {
            assert!(names.contains(&expected), "{expected} missing");
        }
        let ciphers: Vec<&str> = legacy.cipher.iter().map(AsRef::as_ref).collect();
        for expected in ["aes128-cbc", "aes192-cbc", "aes256-cbc", "3des-cbc"] {
            assert!(ciphers.contains(&expected), "{expected} missing");
        }
        let macs: Vec<&str> = legacy.mac.iter().map(AsRef::as_ref).collect();
        assert!(macs.contains(&"hmac-sha1"));
        assert!(macs.contains(&"hmac-sha1-etm@openssh.com"));
        let keys: Vec<String> = legacy.key.iter().map(ToString::to_string).collect();
        assert!(keys.iter().any(|k| k == "ssh-rsa"));
        assert!(keys.iter().any(|k| k == "ssh-dss"));
    }

    #[test]
    fn server_lists_use_server_markers() {
        let server = preferred(CryptoPolicy::Modern, Role::Server);
        assert!(server.kex.contains(&kex::EXTENSION_SUPPORT_AS_SERVER));
        assert!(!server.kex.contains(&kex::EXTENSION_SUPPORT_AS_CLIENT));
    }

    #[test]
    fn legacy_use_is_detected_per_algorithm_family() {
        let modern = negotiated(
            "curve25519-sha256",
            "ssh-ed25519",
            "aes256-ctr",
            "hmac-sha2-256",
        );
        assert!(!modern.uses_legacy());
        assert_eq!(modern.mac().as_deref(), Some("hmac-sha2-256"));

        let cases = [
            negotiated("diffie-hellman-group1-sha1", "ssh-ed25519", "aes256-ctr", "hmac-sha2-256"),
            negotiated("curve25519-sha256", "ssh-rsa", "aes256-ctr", "hmac-sha2-256"),
            negotiated("curve25519-sha256", "ssh-dss", "aes256-ctr", "hmac-sha2-256"),
            negotiated("curve25519-sha256", "ssh-ed25519", "3des-cbc", "hmac-sha2-256"),
            negotiated("curve25519-sha256", "ssh-ed25519", "aes256-ctr", "hmac-sha1"),
        ];
        for case in cases {
            assert!(case.uses_legacy(), "{case:?}");
        }
        // rsa-sha2 host key signatures are modern.
        assert!(
            !negotiated("curve25519-sha256", "rsa-sha2-512", "aes256-ctr", "hmac-sha2-256")
                .uses_legacy()
        );
    }

    #[test]
    fn aead_ciphers_make_the_mac_irrelevant() {
        let aead = negotiated(
            "curve25519-sha256",
            "ssh-ed25519",
            "chacha20-poly1305@openssh.com",
            "hmac-sha1",
        );
        assert!(!aead.uses_legacy());
        assert_eq!(aead.mac(), None);
    }

    #[test]
    fn differing_macs_are_both_reported() {
        let mut names = negotiated("curve25519-sha256", "ssh-ed25519", "aes256-ctr", "hmac-sha2-256");
        names.server_mac = "hmac-sha2-512".into();
        assert_eq!(names.mac().as_deref(), Some("hmac-sha2-256 / hmac-sha2-512"));
    }

    #[test]
    fn mismatch_message_points_at_legacy_mode() {
        let theirs = vec![
            "diffie-hellman-group1-sha1".to_owned(),
            "diffie-hellman-group14-sha1".to_owned(),
            "kex-strict-s-v00@openssh.com".to_owned(),
        ];
        let message = describe_no_common(&AlgorithmKind::Kex, &theirs, CryptoPolicy::Modern);
        assert!(message.contains("no common key exchange algorithm"), "{message}");
        assert!(
            message.contains("diffie-hellman-group1-sha1, diffie-hellman-group14-sha1"),
            "{message}"
        );
        assert!(!message.contains("kex-strict"), "{message}");
        assert!(message.contains("legacy"), "{message}");
        assert!(message.contains("`auto` or `always`"), "{message}");
    }

    #[test]
    fn mismatch_message_admits_unsupported_algorithms() {
        let theirs = vec!["hmac-md5".to_owned(), "hmac-md5-96".to_owned()];
        for policy in [CryptoPolicy::Modern, CryptoPolicy::Legacy] {
            let message = describe_no_common(&AlgorithmKind::Mac, &theirs, policy);
            assert!(message.contains("hmac-md5, hmac-md5-96"), "{message}");
            assert!(message.contains("none of them is supported"), "{message}");
        }
        // Legacy already tried: do not suggest enabling it again.
        let theirs = vec!["3des-cbc".to_owned()];
        let message = describe_no_common(&AlgorithmKind::Cipher, &theirs, CryptoPolicy::Legacy);
        assert!(!message.contains("`auto` or `always`"), "{message}");
    }

    #[test]
    fn client_config_follows_the_policy() {
        let modern = client_config(CryptoPolicy::Modern, Some(Duration::from_secs(30)));
        assert_eq!(modern.keepalive_interval, Some(Duration::from_secs(30)));
        assert_eq!(modern.gex.min_group_size(), 2048);
        assert_eq!(modern.limits.rekey_time_limit, Duration::from_secs(3600));

        let legacy = client_config(CryptoPolicy::Legacy, None);
        assert_eq!(legacy.keepalive_interval, None);
        assert_eq!(legacy.gex.preferred_group_size(), 2048);
        assert!(legacy.limits.rekey_time_limit > Duration::from_secs(86_400 * 365));
    }
}

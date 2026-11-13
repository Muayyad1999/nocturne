//! Recognise how a secret in a device configuration is stored.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::cisco7;

/// The `kind` reported when nothing matches.
pub(crate) const UNKNOWN: &str = "unknown";

/// How a secret is stored, and what that is worth.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SecretKind {
    /// Stable identifier: `cisco_type7`, `md5crypt`, `junos_9`, ..., or `unknown`.
    pub kind: String,
    /// Human-readable name, e.g. `Cisco type 7 (Vigenère obfuscation)`.
    pub name: String,
    /// Whether the original secret can be recovered from this form by design
    /// (plain text, obfuscation, or encryption under a key the device holds).
    pub reversible: bool,
    /// `none` (as good as plain text), `weak`, `moderate`, `strong` or `unknown`.
    pub strength: String,
    /// What to do about it.
    pub advice: String,
}

struct Kind {
    kind: &'static str,
    name: &'static str,
    reversible: bool,
    strength: &'static str,
    advice: &'static str,
}

const CISCO_TYPE0: Kind = Kind {
    kind: "cisco_type0",
    name: "Cisco type 0 (plain text)",
    reversible: true,
    strength: "none",
    advice: "Stored in clear text: anyone who sees the configuration has the secret. For logins use \
             `algorithm-type scrypt secret` (type 9) or `sha256` (type 8); for keys the device must replay, type 6 (AES).",
};
const CISCO_TYPE4: Kind = Kind {
    kind: "cisco_type4",
    name: "Cisco type 4 (unsalted SHA-256)",
    reversible: false,
    strength: "weak",
    advice: "Withdrawn by Cisco in 2013: the implementation uses a single SHA-256 pass and no salt, so it is \
             faster to brute-force than type 5. Re-enter the secret as type 8 or type 9.",
};
const CISCO_TYPE5: Kind = Kind {
    kind: "cisco_type5",
    name: "Cisco type 5 (MD5-crypt)",
    reversible: false,
    strength: "weak",
    advice: "Salted MD5 with 1,000 rounds, fast to brute-force on current hardware. Migrate to type 8 \
             (`algorithm-type sha256`) or type 9 (`algorithm-type scrypt`).",
};
const CISCO_TYPE6: Kind = Kind {
    kind: "cisco_type6",
    name: "Cisco type 6 (AES, reversible with the master key)",
    reversible: true,
    strength: "moderate",
    advice: "AES encryption under the device's master key (`key config-key password-encrypt`), meant for secrets \
             the device has to read back such as pre-shared keys. Anyone holding the master key can decrypt it, \
             so keep that key out of backups and tickets.",
};
const CISCO_TYPE7: Kind = Kind {
    kind: "cisco_type7",
    name: "Cisco type 7 (Vigenère obfuscation)",
    reversible: true,
    strength: "none",
    advice: "Not encryption: the key is public and the secret decodes instantly (this toolbox does it). Treat it \
             as clear text; use `secret` (type 8 or 9) for logins and type 6 (AES) for keys where the platform supports it.",
};
const CISCO_TYPE8: Kind = Kind {
    kind: "cisco_type8",
    name: "Cisco type 8 (PBKDF2-SHA-256)",
    reversible: false,
    strength: "strong",
    advice: "PBKDF2-HMAC-SHA-256 with 20,000 iterations and a salt. A sound choice; what matters now is the \
             strength of the password itself.",
};
const CISCO_TYPE9: Kind = Kind {
    kind: "cisco_type9",
    name: "Cisco type 9 (scrypt)",
    reversible: false,
    strength: "strong",
    advice: "scrypt, a memory-hard hash and the most expensive to attack that IOS offers. What matters now is \
             the strength of the password itself.",
};
const CISCO_PIX_MD5: Kind = Kind {
    kind: "cisco_pix_md5",
    name: "Cisco PIX/ASA `encrypted` (unsalted MD5)",
    reversible: false,
    strength: "weak",
    advice: "The legacy PIX hash: one unsalted MD5 pass over at most 16 characters. Upgrade to an ASA release \
             that stores PBKDF2 hashes (the `pbkdf2` keyword) and re-enter the password.",
};
const CISCO_ASA_PBKDF2: Kind = Kind {
    kind: "cisco_asa_pbkdf2",
    name: "Cisco ASA PBKDF2-SHA-512",
    reversible: false,
    strength: "strong",
    advice: "PBKDF2-HMAC-SHA-512 with a salt. A sound choice; what matters now is the strength of the password itself.",
};
const MD5CRYPT: Kind = Kind {
    kind: "md5crypt",
    name: "MD5-crypt ($1$)",
    reversible: false,
    strength: "weak",
    advice: "Salted MD5 with 1,000 rounds (Cisco type 5, older Junos and Linux), fast to brute-force on current \
             hardware. Re-hash with SHA-512-crypt or better (Junos: `set system login password format sha512`).",
};
const SHA256CRYPT: Kind = Kind {
    kind: "sha256crypt",
    name: "SHA-256-crypt ($5$)",
    reversible: false,
    strength: "strong",
    advice: "Salted, iterated SHA-256 (5,000 rounds unless stated). Fine with a strong password.",
};
const SHA512CRYPT: Kind = Kind {
    kind: "sha512crypt",
    name: "SHA-512-crypt ($6$)",
    reversible: false,
    strength: "strong",
    advice: "Salted, iterated SHA-512 (5,000 rounds unless stated), the default on current Junos, EOS and Linux. \
             Fine with a strong password.",
};
const BCRYPT: Kind = Kind {
    kind: "bcrypt",
    name: "bcrypt ($2$)",
    reversible: false,
    strength: "strong",
    advice: "An adaptive hash; the two digits after the prefix are the cost (2^cost rounds). Fine with a strong \
             password and a cost of 10 or more.",
};
const JUNOS_9: Kind = Kind {
    kind: "junos_9",
    name: "Junos $9$ (reversible obfuscation)",
    reversible: true,
    strength: "none",
    advice: "Not encryption: the algorithm is public and the secret decodes instantly (this toolbox does it). \
             Treat it as clear text, keep configurations private, and consider `set system master-password`, \
             which stores secrets AES-encrypted ($8$) instead.",
};
const JUNOS_8: Kind = Kind {
    kind: "junos_8",
    name: "Junos $8$ (AES-256-GCM under the master password)",
    reversible: true,
    strength: "strong",
    advice: "Encrypted with a key derived from the device's master password. It can only be read back with that \
             password, so store the master password safely and separately from configuration backups.",
};
const HUAWEI_CIPHER: Kind = Kind {
    kind: "huawei_cipher",
    name: "Huawei cipher (reversible encryption)",
    reversible: true,
    strength: "weak",
    advice: "Reversible by design, and on many VRP releases encrypted with a key built into the software, so \
             treat it as readable by anyone who has the configuration. Use `irreversible-cipher` for login passwords.",
};
const HUAWEI_IRREVERSIBLE: Kind = Kind {
    kind: "huawei_irreversible",
    name: "Huawei irreversible-cipher (salted hash)",
    reversible: false,
    strength: "strong",
    advice: "A salted one-way hash; the password cannot be read back. What matters now is the strength of the password itself.",
};
const H3C_CIPHER: Kind = Kind {
    kind: "h3c_cipher",
    name: "H3C Comware cipher ($c$3$, reversible encryption)",
    reversible: true,
    strength: "weak",
    advice: "Reversible by design with a key built into Comware, so treat it as readable by anyone who has the \
             configuration. Use `password hash` for local users where the release supports it.",
};
const H3C_HASH: Kind = Kind {
    kind: "h3c_hash",
    name: "H3C Comware hash ($h$6$, salted SHA-512)",
    reversible: false,
    strength: "moderate",
    advice: "A salted SHA-512 hash; the password cannot be read back, but a weak one can still be guessed quickly. Use a long password.",
};
const FORTIOS_ENC: Kind = Kind {
    kind: "fortios_enc",
    name: "FortiOS ENC (reversible encryption)",
    reversible: true,
    strength: "weak",
    advice: "Reversible encryption. Unless `private-data-encryption` is enabled, FortiOS uses a hard-coded key that \
             is publicly known (CVE-2019-6693), so treat it as clear text and keep configuration backups private.",
};
const FORTIOS_SHA256: Kind = Kind {
    kind: "fortios_hash",
    name: "FortiOS ENC administrator hash (salted SHA-256)",
    reversible: false,
    strength: "weak",
    advice: "One salted SHA-256 pass: one-way, but cheap to brute-force. Use a long password; current FortiOS \
             releases store PBKDF2 hashes (prefix PB2) once the password is set again.",
};
const FORTIOS_SHA1: Kind = Kind {
    kind: "fortios_hash",
    name: "FortiOS ENC administrator hash (salted SHA-1)",
    reversible: false,
    strength: "weak",
    advice: "One salted SHA-1 pass from old FortiOS releases: one-way, but cheap to brute-force. Upgrade and set the password again.",
};
const FORTIOS_PBKDF2: Kind = Kind {
    kind: "fortios_hash",
    name: "FortiOS ENC administrator hash (PBKDF2)",
    reversible: false,
    strength: "strong",
    advice: "A salted, iterated PBKDF2 hash. What matters now is the strength of the password itself.",
};
const ROUTEROS_PLAIN: Kind = Kind {
    kind: "routeros_plain",
    name: "RouterOS export (plain text)",
    reversible: true,
    strength: "none",
    advice: "RouterOS exports secrets in clear text when asked to (`export show-sensitive`). Share exports made \
             with `hide-sensitive` instead and keep full ones as carefully as the passwords themselves.",
};
const SNMP_COMMUNITY: Kind = Kind {
    kind: "snmp_community",
    name: "SNMP community string (plain text)",
    reversible: true,
    strength: "none",
    advice: "SNMP v1/v2c communities are stored and sent in clear text. Prefer SNMPv3 with authentication and \
             privacy; if v2c has to stay, use a long random read-only community restricted by an ACL, never `public` or `private`.",
};
const PLAINTEXT: Kind = Kind {
    kind: "plaintext",
    name: "Plain text",
    reversible: true,
    strength: "none",
    advice: "Stored in clear text: anyone who sees the configuration has the secret. Use the platform's hashed \
             form for logins and its strongest encryption for keys.",
};
const UNKNOWN_KIND: Kind = Kind {
    kind: UNKNOWN,
    name: "Not recognised",
    reversible: false,
    strength: "unknown",
    advice: "This matches no secret format the toolbox knows. If it is a password as typed, it is plain text and \
             should not be stored in a configuration or a ticket.",
};

/// Self-describing value formats, searched for anywhere in the input; the
/// first pattern that matches decides. Hash lengths are upper bounds only,
/// so a value that was cut off when pasting is still recognised.
static VALUE_FORMATS: LazyLock<Vec<(Regex, &'static Kind)>> = LazyLock::new(|| {
    const CRYPT: &str = "[./0-9A-Za-z]";
    let patterns: [(String, &'static Kind); 12] = [
        // Formats with a prefix of their own come first: `$h$6$...` must not
        // be taken for the `$6$...` it contains.
        (r"\$1[ac]\$\S+\$".to_owned(), &HUAWEI_IRREVERSIBLE),
        (r"\$c\$3\$\S+".to_owned(), &H3C_CIPHER),
        (r"\$h\$6\$\S+".to_owned(), &H3C_HASH),
        (r"\$sha512\$[0-9]+\$[A-Za-z0-9+/=]+\$[A-Za-z0-9+/=]+".to_owned(), &CISCO_ASA_PBKDF2),
        (r"\$8\$aes256-gcm\$".to_owned(), &JUNOS_8),
        (format!(r"\$8\${CRYPT}{{1,16}}\${CRYPT}{{1,43}}"), &CISCO_TYPE8),
        // Cisco type 9 has a salt and a hash; a Junos `$9$` secret has no further `$`.
        (format!(r"\$9\${CRYPT}{{1,16}}\${CRYPT}{{1,43}}"), &CISCO_TYPE9),
        (r"\$9\$[./0-9A-Za-z-]{4,}".to_owned(), &JUNOS_9),
        (format!(r"\$1\${CRYPT}{{1,8}}\${CRYPT}{{1,22}}"), &MD5CRYPT),
        (format!(r"\$5\$(?:rounds=[0-9]+\$)?{CRYPT}{{1,16}}\${CRYPT}{{1,43}}"), &SHA256CRYPT),
        (format!(r"\$6\$(?:rounds=[0-9]+\$)?{CRYPT}{{1,16}}\${CRYPT}{{1,86}}"), &SHA512CRYPT),
        (format!(r"\$2[abxy]\$[0-9]{{2}}\${CRYPT}{{1,53}}"), &BCRYPT),
    ];
    patterns
        .into_iter()
        // The patterns are constants; one that failed to compile would simply never match.
        .filter_map(|(pattern, kind)| Regex::new(&pattern).ok().map(|regex| (regex, kind)))
        .collect()
});

/// Words that introduce a secret in IOS-style configurations and may be
/// followed by a type digit.
const SECRET_KEYWORDS: [&str; 12] = [
    "password",
    "secret",
    "key",
    "key-string",
    "md5",
    "ascii",
    "hex",
    "local",
    "remote",
    "pre-shared-key",
    "authentication-key",
    "community",
];

/// RouterOS parameters whose value is a secret (matched as the end of the
/// parameter name, so `wpa2-pre-shared-key` and `authentication-password` count).
const ROUTEROS_SUFFIXES: [&str; 8] =
    ["password", "secret", "passphrase", "pre-shared-key", "preshared-key", "private-key", "auth-key", "authentication-key"];

/// Identify how a secret is stored, from a configuration line or the bare value.
///
/// Recognises Cisco types 0/4/5/6/7/8/9 and the PIX/ASA forms, the crypt(3)
/// hashes (`$1$`, `$5$`, `$6$`, bcrypt) wherever they appear, Junos `$9$` and
/// `$8$`, Huawei and H3C cipher blobs, FortiOS `ENC` values, RouterOS exports
/// and SNMP community lines. Anything else is reported as `unknown`.
pub fn identify(input: &str) -> SecretKind {
    let kind = classify(input);
    SecretKind {
        kind: kind.kind.to_owned(),
        name: kind.name.to_owned(),
        reversible: kind.reversible,
        strength: kind.strength.to_owned(),
        advice: kind.advice.to_owned(),
    }
}

fn classify(input: &str) -> &'static Kind {
    let text = input.trim();
    // Quotes and the trailing `;` of Junos and RouterOS lines are not part of a value.
    let words: Vec<&str> =
        text.split_whitespace().map(|word| word.trim_end_matches(';').trim_matches(['"', '\''])).collect();
    let lower: Vec<String> = words.iter().map(|word| word.to_ascii_lowercase()).collect();
    let following = |index: usize| words.get(index + 1).copied();

    // 1. Values that say what they are.
    if words.iter().any(|word| is_huawei_blob(word)) {
        return &HUAWEI_CIPHER;
    }
    if let Some(value) = words.iter().position(|word| *word == "ENC").and_then(following) {
        return match value.get(..3) {
            Some("SH2") => &FORTIOS_SHA256,
            Some("AK1") => &FORTIOS_SHA1,
            Some("PB2") => &FORTIOS_PBKDF2,
            _ => &FORTIOS_ENC,
        };
    }
    for (pattern, kind) in VALUE_FORMATS.iter() {
        if let Some(found) = pattern.find(text) {
            // `secret 5 $1$...` is MD5-crypt under its Cisco name.
            let after_type_5 = text[..found.start()].split_whitespace().next_back() == Some("5");
            return if kind.kind == MD5CRYPT.kind && after_type_5 { &CISCO_TYPE5 } else { kind };
        }
    }

    // 2. IOS-style `<keyword> <type> <value>`.
    for (index, word) in words.iter().enumerate() {
        let Some(value) = following(index) else { break };
        let after_keyword = index > 0 && SECRET_KEYWORDS.contains(&lower[index - 1].as_str());
        match *word {
            "7" if cisco7::has_type7_shape(value) => return &CISCO_TYPE7,
            "4" if is_type4(value) => return &CISCO_TYPE4,
            "6" if after_keyword => return &CISCO_TYPE6,
            "0" if after_keyword => return &CISCO_TYPE0,
            _ => {}
        }
    }
    // `enable password 8Ry2YjIyt7RRXU24 encrypted`
    if let Some(index) = lower.iter().position(|word| word == "encrypted")
        && index > 0
        && words[index - 1].len() == 16
        && words[index - 1].bytes().all(is_crypt_char)
    {
        return &CISCO_PIX_MD5;
    }
    // Huawei / H3C `password simple <value>`
    if lower.iter().position(|word| word == "simple").and_then(following).is_some() {
        return &PLAINTEXT;
    }

    // 3. Lines that are about a clear-text secret.
    if lower.iter().any(|word| word.contains("snmp")) && lower.iter().any(|word| word.contains("community")) {
        return &SNMP_COMMUNITY;
    }
    let is_routeros_secret = |word: &String| {
        word.split_once('=')
            .is_some_and(|(name, value)| !value.is_empty() && ROUTEROS_SUFFIXES.iter().any(|suffix| name.ends_with(suffix)))
    };
    if lower.iter().any(is_routeros_secret) {
        return &ROUTEROS_PLAIN;
    }

    // 4. A bare value with a telling shape.
    if let [value] = words[..] {
        // Short hex strings decode to something printable too often by chance.
        if cisco7::decode_value(value).is_some_and(|plain| plain.len() >= 4) {
            return &CISCO_TYPE7;
        }
        if is_type4(value) {
            return &CISCO_TYPE4;
        }
    }
    &UNKNOWN_KIND
}

/// `%^%#...%^%#`, `%$%$...%$%$`, `%@%@...%@%@`, `%+%#...%+%#`: a value
/// wrapped in the same four-character `%x%y` marker at both ends.
fn is_huawei_blob(word: &str) -> bool {
    let bytes = word.as_bytes();
    bytes.len() > 8
        && bytes[0] == b'%'
        && bytes[2] == b'%'
        && bytes[1].is_ascii_punctuation()
        && bytes[3].is_ascii_punctuation()
        && bytes[bytes.len() - 4..] == bytes[..4]
}

fn is_crypt_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'/'
}

/// Type 4 is 43 characters of the crypt alphabet (a SHA-256 digest).
fn is_type4(value: &str) -> bool {
    value.len() == 43 && value.bytes().all(is_crypt_char)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(input: &str) -> String {
        identify(input).kind
    }

    #[test]
    fn cisco_types_from_configuration_lines() {
        for (line, expected) in [
            ("enable password 0 cisco123", "cisco_type0"),
            ("username admin password 0 cisco123", "cisco_type0"),
            ("enable secret 4 Rv4kArhts7yA2xd8BD2YTVbts7yA2xd8BD2YTVbts7y", "cisco_type4"),
            ("enable secret 5 $1$mERr$hx5rVt7rPNoS4wqbXKX7m0", "cisco_type5"),
            ("username admin privilege 15 secret 5 $1$mERr$hx5rVt7rPNoS4wqbXKX7m0", "cisco_type5"),
            ("crypto isakmp key 6 FLgBaJHXdYY_AcHZZMgQ_RhTDJXHUBAAB address 192.0.2.1", "cisco_type6"),
            (" pre-shared-key local 6 FLgBaJHXdYY_AcHZZMgQ_RhTDJXHUBAAB", "cisco_type6"),
            ("interface Vlan 6 description uplink", "unknown"),
            ("enable password 7 0822455D0A16", "cisco_type7"),
            (" password 7 0822455D0A16", "cisco_type7"),
            ("neighbor 192.0.2.1 password 7 0822455D0A16", "cisco_type7"),
            (" ip ospf message-digest-key 1 md5 7 0822455D0A16", "cisco_type7"),
            ("tacacs-server key 7 \"0822455D0A16\"", "cisco_type7"),
            ("enable secret 8 $8$dsYGNam3K1SIJO$7nv/35M/qr6t.dVc7UY9zrJDWRVqncHub1PE9UlMQFs", "cisco_type8"),
            ("enable secret 9 $9$nhEmQVczB7dqsO$X.HsgL6x1il0RxkOSSvyQYwucySCt7qFm4v7pqCxkKM", "cisco_type9"),
            ("username admin secret 9 $9$nhEmQVczB7dqsO$X.HsgL6x1il0RxkOSSvyQYwucySCt7qFm4v7pqCxkKM", "cisco_type9"),
            ("enable password 8Ry2YjIyt7RRXU24 encrypted", "cisco_pix_md5"),
            ("username admin password $sha512$5000$Otc8AHuqnOJdIOjWJC9R4A==$Wnlq6V3lTqRq1CNqnXpS1w== pbkdf2", "cisco_asa_pbkdf2"),
        ] {
            assert_eq!(kind(line), expected, "{line}");
        }
    }

    #[test]
    fn cisco_types_from_bare_values() {
        assert_eq!(kind("0822455D0A16"), "cisco_type7");
        assert_eq!(kind("7 0822455D0A16"), "cisco_type7");
        assert_eq!(kind("Rv4kArhts7yA2xd8BD2YTVbts7yA2xd8BD2YTVbts7y"), "cisco_type4");
        assert_eq!(kind("$8$dsYGNam3K1SIJO$7nv/35M/qr6t.dVc7UY9zrJDWRVqncHub1PE9UlMQFs"), "cisco_type8");
        assert_eq!(kind("$9$nhEmQVczB7dqsO$X.HsgL6x1il0RxkOSSvyQYwucySCt7qFm4v7pqCxkKM"), "cisco_type9");
        // Hex that does not decode to text is not type 7.
        assert_eq!(kind("08FFFFFF"), "unknown");
        assert_eq!(kind("0841"), "unknown");
        // Neither is a lone key offset, an ordinary number or a short hex string.
        assert_eq!(kind("08"), "unknown");
        assert_eq!(kind("5"), "unknown");
        assert_eq!(kind("123456"), "unknown");
        // With the type digit in front, the shape is enough.
        assert_eq!(kind("password 7 02050D"), "cisco_type7");
    }

    #[test]
    fn crypt_hashes_wherever_they_appear() {
        for (input, expected) in [
            ("$1$mERr$hx5rVt7rPNoS4wqbXKX7m0", "md5crypt"),
            ("$5$rounds=5000$usesomesillystri$KqJWpanXZHKq2BOB43TSaYhEWsQ1Lr5QNyPCDH/Tp.6", "sha256crypt"),
            ("$5$saltstring$5B8vYYiY.CVt1RlTTf8KbXBH3hsxY/GNooZaBBGWEc5", "sha256crypt"),
            (
                "$6$saltstring$svn8UoSVapNtMuq1ukKS4tPQd8iKwSMHWjl/O817G3uBnIFNjnQJuesI68u4OTLiBFdcbYEdFCoEOfaS35inz1",
                "sha512crypt",
            ),
            ("$2b$12$R9h/cIPz0gi.URNNX3kh2OPST9/PgBkqquzi.Ss7KIUgO2t0jWMUW", "bcrypt"),
            ("$2a$10$N9qo8uLOickgx2ZMRZoMyeIjZAgcfl7p92ldGxad68LJZdL17lhWy", "bcrypt"),
            // A Linux shadow entry.
            ("root:$6$saltstring$svn8UoSVapNtMuq1ukKS4tPQd8iKwSMHWjl/O817G3uBnIFNjnQJuesI68u4OTLiBFdcbYEdFCoEOfaS35inz1:19700:0:99999:7:::", "sha512crypt"),
            // Arista and NX-OS spell the algorithm differently but store the same thing.
            ("username admin privilege 15 role network-admin secret sha512 $6$saltstring$svn8UoSVapNtMuq1ukKS4tPQd8iKwSMHWjl", "sha512crypt"),
            ("username admin password 5 $5$saltstring$5B8vYYiY.CVt1RlTTf8KbXBH3hsxY/GNooZaBBGWEc5 role network-admin", "sha256crypt"),
            // Cut off while pasting.
            ("$1$mERr$hx5rVt7rPN", "md5crypt"),
        ] {
            assert_eq!(kind(input), expected, "{input}");
        }
    }

    #[test]
    fn junos_secrets() {
        for (line, expected) in [
            ("$9$LbHX-wg4Z", "junos_9"),
            ("secret \"$9$LbHX-wg4Z\"; ## SECRET-DATA", "junos_9"),
            ("set system tacplus-server 192.0.2.1 secret \"$9$LbHX-wg4Z\"", "junos_9"),
            ("encrypted-password \"$1$mERr$hx5rVt7rPNoS4wqbXKX7m0\"; ## SECRET-DATA", "md5crypt"),
            ("encrypted-password \"$5$saltstring$5B8vYYiY.CVt1RlTTf8KbXBH3hsxY/GNooZaBBGWEc5\";", "sha256crypt"),
            (
                "set system root-authentication encrypted-password \"$6$saltstring$svn8UoSVapNtMuq1ukKS4tPQd8iKwSMHWjl/O817G3uBnIFNjnQJuesI68u4OTLiBFdcbYEdFCoEOfaS35inz1\"",
                "sha512crypt",
            ),
            (
                "secret \"$8$aes256-gcm$hmac-sha2-256$100$cFFEUU7SLvU$6Nv4yzcqRNMdl0Vd6mrDXw$wkL7N5MUeL2f+RqJmG3N+Q$uAn1f1+BiSrWZYpLHXZZNoiw7cxZ3D0=\";",
                "junos_8",
            ),
        ] {
            assert_eq!(kind(line), expected, "{line}");
        }
        assert!(identify("$9$LbHX-wg4Z").reversible);
        assert_eq!(identify("$9$LbHX-wg4Z").strength, "none");
    }

    #[test]
    fn huawei_and_h3c() {
        for (line, expected) in [
            ("local-user admin password cipher %^%#JvZxR2g8c;a9~FPN~n'$7`DEV&=G(=Et02P/%\\*!%^%#", "huawei_cipher"),
            (" set authentication password cipher %$%$Qp,8Xw(v7X$3$2!T{Y[R,QbE%$%$", "huawei_cipher"),
            ("snmp-agent community read cipher %@%@8P,:Q!j1(6W!cX8=sWO.,3#N%@%@", "huawei_cipher"),
            ("%+%##!!!!!!!!!\"!!!!\"!!!!*!!!!{5r8>vOT`<BLj/p*;z0HoEJ{%+%#", "huawei_cipher"),
            ("local-user admin password irreversible-cipher $1a$C}!N~8Fh@Q$~!2R%5Xa/k7e7X.nZ~i7Sq,Q8v&B(.,v%qI$", "huawei_irreversible"),
            ("local-user admin password simple Huawei@123", "plaintext"),
            ("local-user admin password cipher $c$3$xPfX1bY4Ks2XVUZVUEXSkbcY0c8Yvw==", "h3c_cipher"),
            ("local-user admin password hash $h$6$3BmZtGqU2s1LjQGn$7VhQxhGvcQzvZAf1eL0F1eNhGU5pdKhJ", "h3c_hash"),
        ] {
            assert_eq!(kind(line), expected, "{line}");
        }
    }

    #[test]
    fn fortios() {
        let admin = identify("set password ENC SH2nlSm9QL9tapcHPXIqAXvX7vBJuuqu22hpa0JX0sBuKIo7z2g0Kz/+0KyH4E=");
        assert_eq!((admin.kind.as_str(), admin.reversible), ("fortios_hash", false));
        assert!(admin.name.contains("SHA-256"));
        assert!(identify("set password ENC AK1P6IPcOA4ONEoOaNZ4xHNnonB0q16ZuAwrfzewhnY4CU=").name.contains("SHA-1"));
        assert!(identify("set password ENC PB2aGVsbG8gd29ybGQ=").name.contains("PBKDF2"));
        let psk = identify("set psksecret ENC bHl0aGl1bSBwb2x5bWVyIGJhdHRlcnk=");
        assert_eq!((psk.kind.as_str(), psk.reversible, psk.strength.as_str()), ("fortios_enc", true, "weak"));
        assert!(psk.advice.contains("CVE-2019-6693"));
        assert_eq!(kind("ENC bHl0aGl1bSBwb2x5bWVyIGJhdHRlcnk="), "fortios_enc");
    }

    #[test]
    fn clear_text_lines() {
        for (line, expected) in [
            ("snmp-server community public RO", "snmp_community"),
            ("snmp-server community S3cr3t-RW RW 99", "snmp_community"),
            ("set snmp community public authorization read-only", "snmp_community"),
            ("/snmp community add name=public addresses=10.0.0.0/8", "snmp_community"),
            ("snmp-agent community read public", "snmp_community"),
            ("/user add name=admin password=Sup3rSecret group=full", "routeros_plain"),
            ("/ppp secret add name=alice password=wonderland service=pppoe", "routeros_plain"),
            ("add name=bob secret=\"hunter2\"", "routeros_plain"),
            ("set [ find default=yes ] wpa2-pre-shared-key=correcthorsebattery", "routeros_plain"),
            ("/interface wireguard add name=wg0 private-key=\"yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=\"", "routeros_plain"),
        ] {
            assert_eq!(kind(line), expected, "{line}");
        }
        assert_eq!(identify("snmp-server community public RO").strength, "none");
    }

    #[test]
    fn unknown_input_is_reported_as_such() {
        for input in ["", "   ", "hunter2", "interface GigabitEthernet0/1", "hostname core-sw-01", "password=", "name=public", "🔑🔑🔑"] {
            let found = identify(input);
            assert_eq!((found.kind.as_str(), found.strength.as_str(), found.reversible), ("unknown", "unknown", false), "{input}");
        }
    }

    #[test]
    fn every_kind_is_fully_described() {
        for kind in [
            &CISCO_TYPE0, &CISCO_TYPE4, &CISCO_TYPE5, &CISCO_TYPE6, &CISCO_TYPE7, &CISCO_TYPE8, &CISCO_TYPE9, &CISCO_PIX_MD5,
            &CISCO_ASA_PBKDF2, &MD5CRYPT, &SHA256CRYPT, &SHA512CRYPT, &BCRYPT, &JUNOS_9, &JUNOS_8, &HUAWEI_CIPHER,
            &HUAWEI_IRREVERSIBLE, &H3C_CIPHER, &H3C_HASH, &FORTIOS_ENC, &FORTIOS_SHA256, &FORTIOS_SHA1, &FORTIOS_PBKDF2,
            &ROUTEROS_PLAIN, &SNMP_COMMUNITY, &PLAINTEXT, &UNKNOWN_KIND,
        ] {
            assert!(kind.kind.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'), "{}", kind.kind);
            assert!(!kind.name.is_empty() && kind.advice.ends_with('.'), "{}", kind.kind);
            assert!(["none", "weak", "moderate", "strong", "unknown"].contains(&kind.strength), "{}", kind.kind);
            // What can be read back is never rated above "moderate" unless a separate key protects it.
            if kind.strength == "none" {
                assert!(kind.reversible, "{}", kind.kind);
            }
        }
        assert_eq!(VALUE_FORMATS.len(), 12, "every value pattern compiles");
    }
}

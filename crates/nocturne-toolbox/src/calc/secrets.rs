//! Secrets in device configurations: decoding the reversible encodings,
//! saying plainly which ones those are, and making new passwords and keys.
//!
//! Inheriting a network means reading configurations full of `password 7 …`
//! and `$9$…`, and both of those are **obfuscation, not encryption**: the
//! algorithm is public, there is no key, and `nocturne_tools::secrets` undoes
//! them with arithmetic. A hash cannot be undone, and no method here pretends
//! otherwise: `tools.secret.decode` of a type 5 refuses and says what it is
//! looking at, `tools.secret.identify` carries `reversible` for every form it
//! knows, and `decodable` says which of those this toolbox can actually undo.
//!
//! Nothing in this module writes what it produced or recovered anywhere. A
//! decoded password goes to the caller and to nobody else — not the audit
//! trail, not the job log, not the probe history, not a model's context.

use nocturne_app::{App, AppError, AppResult, Router, util};
use nocturne_tools::secrets::{self, PasswordPolicy, SecretKind};
use nocturne_transport::KeyKind;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::errors::tool;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct DecodeParams {
    /// `cisco7` or `junos9`; left out, the encoding is recognised from the text.
    pub kind: Option<String>,
    /// The encoded value, or the whole configuration line it sits in.
    pub text: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct EncodeParams {
    pub kind: String,
    /// The plain text to obfuscate.
    pub text: String,
    /// Cisco type 7 only: the salt, 0 to 15 (random when left out).
    pub salt: Option<u8>,
}

#[derive(Debug, Deserialize)]
pub struct TextParams {
    pub text: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PasswordParams {
    pub length: Option<usize>,
    pub upper: Option<bool>,
    pub lower: Option<bool>,
    pub digits: Option<bool>,
    pub symbols: Option<bool>,
    /// Leave out characters that are easily confused (`0 O o 1 l I . ,`).
    pub avoid_ambiguous: Option<bool>,
    /// How many to generate (1 to 50).
    pub count: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct KeyParams {
    /// `ed25519` (the default), `rsa3072` or `rsa4096`.
    pub kind: Option<String>,
    pub comment: String,
}

#[derive(Debug, Serialize)]
pub struct Decoded {
    /// The encoding that was undone: `cisco7` or `junos9`.
    pub kind: &'static str,
    pub name: &'static str,
    pub secret: String,
    /// Why this was possible at all, in words a ticket can quote.
    pub note: &'static str,
}

#[derive(Debug, Serialize)]
pub struct Encoded {
    pub kind: &'static str,
    pub encoded: String,
    /// The configuration line this value belongs in.
    pub example: String,
    pub note: &'static str,
}

#[derive(Debug, Serialize)]
pub struct Identified {
    #[serde(flatten)]
    pub kind: SecretKind,
    /// Whether this toolbox can recover the secret from this form.
    pub decodable: bool,
    /// The `kind` to pass to `tools.secret.decode`, when it can.
    pub decoder: Option<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct Passwords {
    pub passwords: Vec<String>,
    /// The symbols the generator may use, so a device's restrictions can be checked.
    pub symbols: &'static str,
}

const CISCO7_NOTE: &str = "Cisco type 7 is Vigenère obfuscation with a published key, not encryption: anyone with the configuration has the secret. Use `secret` (type 8 or 9) for logins.";
const JUNOS9_NOTE: &str = "Junos `$9$` is obfuscation with a published algorithm, not encryption: anyone with the configuration has the secret. Use a hashed format for logins.";

/// Which decoder a `kind` parameter names, or `None` for a name that is not
/// one of the two.
fn decoder_known(kind: &str) -> Option<&'static str> {
    match kind
        .trim()
        .to_ascii_lowercase()
        .replace(['-', '_', ' '], "")
        .as_str()
    {
        "cisco7" | "ciscotype7" | "type7" | "7" => Some("cisco7"),
        "junos9" | "junos" | "juniper9" | "9" => Some("junos9"),
        _ => None,
    }
}

/// What to say about a named encoding this toolbox cannot undo.
///
/// Saying "everything else is a hash" would be false for the interesting
/// case: Cisco type 6 is AES, reversible by design, and the only reason it
/// cannot be undone here is that the key lives on the device. That
/// distinction is the whole point of `decodable` in `tools.secret.identify`,
/// so this refusal must not flatten it.
fn not_a_decoder(named: &str) -> String {
    format!(
        "`{named}` is not an encoding this toolbox can undo. The two it can are `cisco7` (Cisco type 7) and `junos9` (Junos $9$) — both obfuscation with a published algorithm. Everything else is either a hash, which cannot be decoded at all, or reversible only with a key the device holds; `tools.secret.identify` says which, in `decodable`."
    )
}

fn decoder_named(kind: &str) -> AppResult<&'static str> {
    decoder_known(kind).ok_or_else(|| AppError::invalid(not_a_decoder(kind.trim())))
}

/// The decoder for a secret that was recognised rather than named, or the
/// reason there is none.
fn decoder_for(found: &SecretKind) -> Result<&'static str, String> {
    match found.kind.as_str() {
        "cisco_type7" => Ok("cisco7"),
        "junos_9" => Ok("junos9"),
        "cisco_type0" | "routeros_plain" | "plaintext" | "snmp_community" => Err(format!(
            "that is {}: the secret is the text itself, there is nothing to decode.",
            found.name
        )),
        "unknown" => Err(
            "that matches no secret format this toolbox knows, so there is nothing to decode. If it is a password as typed, it is already plain text."
                .to_string(),
        ),
        _ if found.reversible => Err(format!(
            "that is {}. It is reversible by design, but only with a key the device holds, which Nocturne does not have. {}",
            found.name, found.advice
        )),
        _ => Err(format!(
            "that is {}: a hash, which cannot be decoded — only guessed at with a cracker. {}",
            found.name, found.advice
        )),
    }
}

fn decode(params: DecodeParams) -> AppResult<Decoded> {
    if params.text.trim().is_empty() {
        return Err(AppError::invalid(
            "give the encoded value, or the configuration line it is on",
        ));
    }
    let found = secrets::identify(&params.text);
    let kind = match params.kind.as_deref() {
        Some(named) => match decoder_known(named) {
            Some(kind) => kind,
            // A name this toolbox cannot act on. What is true of *this text*
            // is more useful than a sentence about names, and `decoder_for`
            // already draws the hash / reversible-with-a-key distinction —
            // so let it answer whenever the text is something recognisable.
            None => {
                let why = decoder_for(&found)
                    .err()
                    .unwrap_or_else(|| not_a_decoder(named));
                return Err(
                    AppError::invalid(why).with_details(json!({"identified": found.clone()}))
                );
            }
        },
        // Nothing was named: what the text is decides, and if it is a hash
        // the answer is which hash, not a failed attempt at arithmetic.
        None => decoder_for(&found).map_err(|why| {
            AppError::invalid(why).with_details(json!({"identified": found.clone()}))
        })?,
    };
    // Both decoders find their value inside a configuration line themselves.
    let secret = match kind {
        "cisco7" => secrets::cisco7_decode(&params.text),
        _ => secrets::junos9_decode(&params.text),
    }
    .map_err(tool)?;
    // Deliberately no audit entry and no log line: see the module note.
    Ok(Decoded {
        kind,
        name: if kind == "cisco7" {
            "Cisco type 7 (Vigenère obfuscation)"
        } else {
            "Junos $9$ (obfuscation)"
        },
        secret,
        note: if kind == "cisco7" {
            CISCO7_NOTE
        } else {
            JUNOS9_NOTE
        },
    })
}

fn encode(params: EncodeParams) -> AppResult<Encoded> {
    if params.text.is_empty() {
        return Err(AppError::invalid("give the text to encode"));
    }
    let kind = decoder_named(&params.kind)?;
    match kind {
        "cisco7" => {
            let salt = match params.salt {
                Some(salt) if salt > 15 => {
                    return Err(AppError::invalid(
                        "the Cisco type 7 salt is a number from 0 to 15",
                    ));
                }
                Some(salt) => salt,
                None => {
                    let mut byte = [0u8; 1];
                    util::random_bytes(&mut byte);
                    byte[0] % 16
                }
            };
            let encoded = secrets::cisco7_encode(&params.text, salt).map_err(tool)?;
            Ok(Encoded {
                kind,
                example: format!("username admin password 7 {encoded}"),
                encoded,
                note: CISCO7_NOTE,
            })
        }
        _ => {
            let mut seed = [0u8; 32];
            util::random_bytes(&mut seed);
            let encoded = secrets::junos9_encode(&params.text, &seed).map_err(tool)?;
            Ok(Encoded {
                kind,
                example: format!(
                    "set system login user admin authentication encrypted-password \"{encoded}\""
                ),
                encoded,
                note: JUNOS9_NOTE,
            })
        }
    }
}

fn identify(text: &str) -> Identified {
    let kind = secrets::identify(text);
    let decoder = decoder_for(&kind).ok();
    Identified {
        decodable: decoder.is_some(),
        decoder,
        kind,
    }
}

fn passwords(params: PasswordParams) -> AppResult<Passwords> {
    let policy = PasswordPolicy {
        length: params.length.unwrap_or(20),
        upper: params.upper.unwrap_or(true),
        lower: params.lower.unwrap_or(true),
        digits: params.digits.unwrap_or(true),
        symbols: params.symbols.unwrap_or(true),
        avoid_ambiguous: params.avoid_ambiguous.unwrap_or(false),
    };
    let count = params.count.unwrap_or(1).clamp(1, 50);
    let mut passwords = Vec::with_capacity(count);
    for _ in 0..count {
        // Fresh entropy per password, from the operating system: the engine is
        // a pure function of the seed, so the seed is where the randomness
        // has to come from.
        let mut seed = [0u8; 32];
        util::random_bytes(&mut seed);
        passwords.push(secrets::generate_password(&policy, &seed).map_err(tool)?);
    }
    Ok(Passwords {
        passwords,
        symbols: secrets::PASSWORD_SYMBOLS,
    })
}

fn key_kind(name: Option<&str>) -> AppResult<(KeyKind, &'static str)> {
    match name
        .unwrap_or("ed25519")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "ed25519" => Ok((KeyKind::Ed25519, "ed25519")),
        "rsa3072" => Ok((KeyKind::Rsa3072, "rsa3072")),
        "rsa4096" => Ok((KeyKind::Rsa4096, "rsa4096")),
        other => Err(AppError::invalid(format!(
            "`{other}` is not a key type Nocturne generates; use `ed25519` (recommended), `rsa3072` or `rsa4096`"
        ))),
    }
}

/// Generate a key pair, preferring whoever already owns that job.
///
/// The terminal feature registers `keys.generate` and computes the
/// `SHA256:…` fingerprint; features may not link to each other but they may
/// call each other by name, so that is what this does. In a build without the
/// terminal feature the engine is called directly and the answer says that the
/// fingerprint was not computed rather than inventing one.
async fn keys(app: &App, params: KeyParams) -> AppResult<Value> {
    let (kind, name) = key_kind(params.kind.as_deref())?;
    let comment = params.comment.trim().to_string();
    if comment.contains(['\n', '\r']) {
        return Err(AppError::invalid(
            "the comment on a key is a single line (it becomes the end of the public key line)",
        ));
    }
    match app
        .rpc(
            "keys.generate",
            json!({"kind": name, "comment": comment.clone()}),
        )
        .await
    {
        Ok(generated) => Ok(generated),
        Err(absent) if absent.code == "unknown_method" => {
            // RSA generation is seconds of arithmetic: off the worker threads.
            let generated = tokio::task::spawn_blocking(move || {
                nocturne_transport::generate_keypair(kind, &comment)
            })
            .await
            .map_err(|err| AppError::internal(format!("key generation did not finish: {err}")))?;
            let (private_key, public_key) = generated.map_err(AppError::from)?;
            Ok(json!({
                "private_key": private_key,
                "public_key": public_key,
                "kind": name,
                "fingerprint": Value::Null,
            }))
        }
        Err(other) => Err(other),
    }
}

pub fn register(router: &mut Router) {
    router.add(
        "tools.secret.decode",
        |_: App, params: DecodeParams| async move { decode(params) },
    );
    router.add(
        "tools.secret.encode",
        |_: App, params: EncodeParams| async move { encode(params) },
    );
    router.add(
        "tools.secret.identify",
        |_: App, params: TextParams| async move { Ok(identify(&params.text)) },
    );
    router.add(
        "tools.password",
        |_: App, params: PasswordParams| async move { passwords(params) },
    );
    router.add(
        "tools.keys.generate",
        |app: App, params: KeyParams| async move { keys(&app, params).await },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reversible_encodings_are_named_and_the_rest_are_refused() {
        // `0822455D0A16` is `cisco`, from Cisco's own documentation and every
        // type-7 cracker ever written.
        let decoded = decode(DecodeParams {
            kind: None,
            text: "username admin password 7 0822455D0A16".into(),
        })
        .unwrap();
        assert_eq!((decoded.kind, decoded.secret.as_str()), ("cisco7", "cisco"));
        assert!(decoded.note.contains("not encryption"));

        // A type 5 hash: the refusal says what it is and why it stays that way.
        let refused = decode(DecodeParams {
            kind: None,
            text: "enable secret 5 $1$mERr$3GCGQMcGNJZPAJLtgvmgO0".into(),
        })
        .unwrap_err();
        assert_eq!(refused.code, "invalid");
        assert!(
            refused.message.contains("hash") && refused.message.contains("cannot be decoded"),
            "{}",
            refused.message
        );
        assert_eq!(
            refused.details.unwrap()["identified"]["reversible"],
            false,
            "the refusal carries what it identified"
        );

        // Type 6 is reversible — but only with the device's master key.
        let master_key = decode(DecodeParams {
            kind: None,
            text: "crypto isakmp key 6 FLgBaJHXdYY_AcHZZMgQ_RhTDJXHUBAAB address 192.0.2.1".into(),
        })
        .unwrap_err();
        assert!(
            master_key.message.contains("key the device holds"),
            "{}",
            master_key.message
        );

        assert_eq!(
            decoder_named("sha512").unwrap_err().code,
            "invalid",
            "a hash is not a decoder"
        );
    }

    #[test]
    fn identification_says_reversible_and_decodable_separately() {
        let seven = identify("password 7 0822455D0A16");
        assert!(seven.kind.reversible && seven.decodable);
        assert_eq!(seven.decoder, Some("cisco7"));

        let nine = identify("$9$abcABC123");
        assert_eq!(nine.kind.kind, "junos_9");
        assert_eq!(nine.decoder, Some("junos9"));

        let five = identify("$1$mERr$3GCGQMcGNJZPAJLtgvmgO0");
        assert!(!five.kind.reversible && !five.decodable && five.decoder.is_none());

        let six = identify(" pre-shared-key local 6 FLgBaJHXdYY_AcHZZMgQ_RhTDJXHUBAAB");
        assert!(
            six.kind.reversible && !six.decodable,
            "reversible by the device is not decodable here"
        );
    }

    #[test]
    fn encoding_round_trips_and_the_salt_is_bounded() {
        let encoded = encode(EncodeParams {
            kind: "cisco7".into(),
            text: "lab-only".into(),
            salt: Some(4),
        })
        .unwrap();
        assert!(encoded.encoded.starts_with("04"), "{}", encoded.encoded);
        assert_eq!(
            secrets::cisco7_decode(&encoded.encoded).unwrap(),
            "lab-only"
        );
        assert!(encoded.example.contains("password 7 "));

        assert_eq!(
            encode(EncodeParams {
                kind: "cisco7".into(),
                text: "x".into(),
                salt: Some(16),
            })
            .unwrap_err()
            .code,
            "invalid"
        );
        // A random salt still round-trips, which is the only thing that matters.
        let random = encode(EncodeParams {
            kind: "junos9".into(),
            text: "lab-only".into(),
            salt: None,
        })
        .unwrap();
        assert_eq!(secrets::junos9_decode(&random.encoded).unwrap(), "lab-only");
    }

    #[test]
    fn generated_passwords_follow_the_policy_and_differ_from_each_other() {
        let made = passwords(PasswordParams {
            length: Some(24),
            symbols: Some(false),
            count: Some(3),
            ..PasswordParams::default()
        })
        .unwrap();
        assert_eq!(made.passwords.len(), 3);
        assert!(made.passwords.iter().all(|word| word.chars().count() == 24));
        assert!(
            made.passwords
                .iter()
                .all(|word| word.chars().all(|c| c.is_ascii_alphanumeric())),
            "{:?}",
            made.passwords
        );
        assert_ne!(made.passwords[0], made.passwords[1]);

        assert_eq!(
            passwords(PasswordParams {
                length: Some(0),
                ..PasswordParams::default()
            })
            .unwrap_err()
            .code,
            "invalid"
        );
    }

    #[test]
    fn key_types_are_named_as_the_rest_of_nocturne_names_them() {
        assert!(matches!(key_kind(None), Ok((KeyKind::Ed25519, "ed25519"))));
        assert!(matches!(
            key_kind(Some(" RSA4096 ")),
            Ok((KeyKind::Rsa4096, "rsa4096"))
        ));
        let refused = key_kind(Some("dsa")).unwrap_err();
        assert_eq!(refused.code, "invalid");
        assert!(refused.message.contains("ed25519"));
    }
}

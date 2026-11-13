//! Generating an SSH key pair.
//!
//! The pair is returned once and kept nowhere: the public half goes onto the
//! devices, the private half into a credential (`credentials.create`) or a
//! file of the user's choosing. Nocturne does not quietly keep a copy, and
//! the private key never reaches the audit trail or the logs.

use nocturne_app::{App, AppError, AppResult, Router};
use nocturne_transport::KeyKind;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Longest comment put into a key. OpenSSH tolerates more, but a comment is
/// a label (`ops@nocturne`), not a document.
const MAX_COMMENT: usize = 200;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GenerateParams {
    /// `ed25519` (the default), `rsa3072` or `rsa4096`.
    kind: Option<String>,
    /// Appended to the public key line, to tell keys apart on a device.
    comment: String,
}

#[derive(Debug, Serialize)]
struct GeneratedKey {
    /// OpenSSH format, unencrypted. Store it in a credential or a file.
    private_key: String,
    /// The `ssh-ed25519 AAAA… comment` line for `authorized_keys`.
    public_key: String,
    kind: String,
    fingerprint: Option<String>,
}

fn kind_of(name: Option<&str>) -> AppResult<(KeyKind, &'static str)> {
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

/// The `SHA256:…` fingerprint of a public key line, as `ssh-keygen -l` prints it.
fn fingerprint(public_key: &str) -> Option<String> {
    use base64::Engine as _;
    use sha2::Digest as _;
    let encoded = public_key.split_whitespace().nth(1)?;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let digest = sha2::Sha256::digest(&raw);
    let text = base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest);
    Some(format!("SHA256:{text}"))
}

async fn generate(app: &App, params: GenerateParams) -> AppResult<GeneratedKey> {
    let (kind, name) = kind_of(params.kind.as_deref())?;
    let comment = params.comment.trim().to_string();
    if comment.chars().count() > MAX_COMMENT {
        return Err(AppError::invalid(format!(
            "the comment on a key is at most {MAX_COMMENT} characters"
        )));
    }
    if comment.contains(['\n', '\r']) {
        return Err(AppError::invalid(
            "the comment on a key is a single line (it becomes the end of the public key line)",
        ));
    }
    // RSA generation is seconds of arithmetic: off the runtime's worker.
    let generated =
        tokio::task::spawn_blocking(move || nocturne_transport::generate_keypair(kind, &comment))
            .await
            .map_err(|err| AppError::internal(format!("key generation did not finish: {err}")))?;
    let (private_key, public_key) = generated.map_err(AppError::from)?;
    let fingerprint = fingerprint(&public_key);
    // The key material itself is deliberately absent from the trail.
    app.audit(
        "keys.generate",
        None,
        format!("Generated an SSH key pair ({name})"),
        json!({"kind": name, "fingerprint": fingerprint}),
    );
    Ok(GeneratedKey {
        private_key,
        public_key,
        kind: name.to_string(),
        fingerprint,
    })
}

pub(crate) fn register(router: &mut Router) {
    router.add(
        "keys.generate",
        |app: App, params: GenerateParams| async move { generate(&app, params).await },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_types_are_named_as_the_specification_says() {
        assert!(matches!(kind_of(None), Ok((KeyKind::Ed25519, "ed25519"))));
        assert!(matches!(
            kind_of(Some(" RSA4096 ")),
            Ok((KeyKind::Rsa4096, "rsa4096"))
        ));
        let err = kind_of(Some("dsa")).unwrap_err();
        assert_eq!(err.code, "invalid");
        assert!(err.message.contains("ed25519"), "{}", err.message);
    }

    #[test]
    fn the_fingerprint_is_of_the_key_not_the_line() {
        // ssh-keygen -lf of this key prints SHA256:…; the comment must not change it.
        let line =
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJ8KxYvRz4yFJkZ0Qm5V2rXy1x2v3w4y5z6A7B8C9D0E";
        let bare = fingerprint(line).expect("fingerprint");
        let commented = fingerprint(&format!("{line} ops@nocturne")).expect("fingerprint");
        assert_eq!(bare, commented);
        assert!(bare.starts_with("SHA256:") && !bare.ends_with('='));
        assert_eq!(fingerprint("not a key"), None);
    }
}

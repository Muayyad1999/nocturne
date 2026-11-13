//! The access token: what it may look like, how one is made, and the file
//! that keeps it between runs (`<data-dir>/server.token`).

use std::fs;
use std::io;
use std::path::Path;

use anyhow::{Context, bail};
use rand::TryRngCore;
use rand::rngs::OsRng;

use crate::private_file;

/// Name of the token file inside the data directory.
pub const FILE_NAME: &str = "server.token";

const MAX_LENGTH: usize = 512;

/// A new random token: 32 bytes from the operating system, as hex.
///
/// # Panics
///
/// If the operating system cannot provide random bytes. Serving with a
/// predictable token would be worse than not starting.
pub fn generate() -> String {
    let mut bytes = [0u8; 32];
    OsRng
        .try_fill_bytes(&mut bytes)
        .expect("the operating system's random number generator is not available");
    hex::encode(bytes)
}

/// Check that `token` can be used as an access token.
///
/// It travels in an `Authorization` header, in the `?token=` query of the
/// event stream and in the `#token=` fragment of the link people open, so it
/// is limited to characters that mean the same in all three without encoding.
pub fn validate(token: &str) -> anyhow::Result<()> {
    if token.is_empty() {
        bail!("the token is empty");
    }
    if token.len() > MAX_LENGTH {
        bail!("the token is longer than {MAX_LENGTH} characters");
    }
    if let Some(unexpected) = token
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || "-._~".contains(*c)))
    {
        bail!(
            "the token may only contain letters, digits, `-`, `.`, `_` and `~` because it is \
             carried in URLs, but it contains {unexpected:?}"
        );
    }
    Ok(())
}

/// The token stored in `data_dir`, created on first use.
pub fn load_or_create(data_dir: &Path) -> anyhow::Result<String> {
    let path = data_dir.join(FILE_NAME);
    let token = generate();
    match private_file::create_new(&path, format!("{token}\n").as_bytes()) {
        Ok(()) => Ok(token),
        // Somebody (an earlier run, or another process just now) was first: theirs is the token.
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => load(data_dir)?
            .with_context(|| format!("the token file `{}` disappeared", path.display())),
        Err(err) => Err(err)
            .with_context(|| format!("could not create the token file `{}`", path.display())),
    }
}

/// The token stored in `data_dir`, or `None` if there is no token file yet.
pub fn load(data_dir: &Path) -> anyhow::Result<Option<String>> {
    let path = data_dir.join(FILE_NAME);
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err)
                .with_context(|| format!("could not read the token file `{}`", path.display()));
        }
    };
    match private_file::restrict(&path) {
        Ok(true) => tracing::warn!(
            file = %path.display(),
            "the token file was readable by other users; its permissions are now 0600"
        ),
        Ok(false) => {}
        Err(err) => tracing::warn!(
            file = %path.display(),
            %err,
            "the token file is readable by other users and its permissions could not be changed"
        ),
    }
    let token = content.trim();
    validate(token).with_context(|| {
        format!(
            "the token file `{}` does not hold a usable token; delete it and a new one is \
             generated on the next start",
            path.display()
        )
    })?;
    Ok(Some(token.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_64_hex_characters_and_unique() {
        let first = generate();
        let second = generate();
        assert_eq!(first.len(), 64);
        assert!(
            first.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "{first}"
        );
        assert_ne!(first, second);
        validate(&first).unwrap();
    }

    #[test]
    fn validation_accepts_url_safe_tokens_only() {
        for good in ["a", "e2e-token", "A.b_c~d-9", &"x".repeat(512)] {
            validate(good).unwrap();
        }
        for bad in [
            "",
            "with space",
            "plus+sign",
            "slash/",
            "equals=",
            "percent%41",
            "hash#",
            "amp&",
            "question?",
            "quote\"",
            "new\nline",
            "ünïcode",
            &"x".repeat(513),
        ] {
            assert!(validate(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn the_token_file_is_created_once_and_reused() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()).unwrap(), None);

        let created = load_or_create(dir.path()).unwrap();
        assert_eq!(created.len(), 64);
        let on_disk = fs::read_to_string(dir.path().join(FILE_NAME)).unwrap();
        assert_eq!(on_disk, format!("{created}\n"));

        assert_eq!(load_or_create(dir.path()).unwrap(), created);
        assert_eq!(load(dir.path()).unwrap(), Some(created));
    }

    #[cfg(unix)]
    #[test]
    fn the_token_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        load_or_create(dir.path()).unwrap();
        let path = dir.path().join(FILE_NAME);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        // A file that was opened up (copied, restored from a backup) is closed again when read.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        load(dir.path()).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn a_hand_written_token_file_is_used_as_is() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(FILE_NAME), "  my-own-token \n").unwrap();
        assert_eq!(load_or_create(dir.path()).unwrap(), "my-own-token");
    }

    #[test]
    fn a_damaged_token_file_is_reported_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(FILE_NAME), "").unwrap();
        let err = format!("{:#}", load_or_create(dir.path()).unwrap_err());
        assert!(
            err.contains(FILE_NAME) && err.contains("delete it"),
            "{err}"
        );
        assert_eq!(fs::read_to_string(dir.path().join(FILE_NAME)).unwrap(), "");
    }
}

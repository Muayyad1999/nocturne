//! The credential vault.
//!
//! Every secret Nocturne keeps — device passwords, enable secrets, private
//! keys, SNMP communities, API keys — is encrypted with XChaCha20-Poly1305
//! under a random 256-bit data key before it touches the database. The secret's
//! id is bound in as associated data, so a ciphertext cannot be moved to
//! another record.
//!
//! The data key itself lives in `vault.key` next to the database, in one of
//! two forms:
//!
//! * **Unprotected** (the default): the key is stored as is, protected only by
//!   file permissions (0600 on Unix). This stops secrets leaking through a
//!   copied database or a backup of it, but not someone who can read both files.
//! * **Password-protected**: the key is wrapped with a key derived from a
//!   master password (Argon2id). The vault starts **locked** and nothing that
//!   needs a credential works until the password is entered.
//!
//! Secrets never leave this module except through [`Vault::get`], and callers
//! are expected to hand them straight to a transport, never to the UI, a log
//! or a model.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use argon2::Argon2;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::{AppError, AppResult};
use crate::store::Store;
use crate::util::random_bytes;

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 24;

#[derive(Serialize, Deserialize)]
struct KeyFile {
    version: u32,
    /// `plain` or `password`.
    mode: String,
    /// Base64 data key (plain mode).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    /// Argon2id salt (password mode).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    salt: Option<String>,
    /// Data key wrapped under the password-derived key: nonce ‖ ciphertext.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wrapped: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VaultState {
    /// Usable; the key file is not password-protected.
    Open,
    /// Usable; a master password protects the key file.
    Unlocked,
    /// A master password is needed before any secret can be read or written.
    Locked,
}

pub struct Vault {
    store: Arc<Store>,
    /// `None` keeps the key in memory only (tests, in-memory databases).
    key_path: Option<PathBuf>,
    key: RwLock<Option<Zeroizing<[u8; KEY_LEN]>>>,
    protected: RwLock<bool>,
}

impl Vault {
    /// Open the vault whose key file is `key_path`, creating a key on first run.
    pub fn open(store: Arc<Store>, key_path: Option<&Path>) -> AppResult<Vault> {
        let vault = Vault {
            store,
            key_path: key_path.map(Path::to_path_buf),
            key: RwLock::new(None),
            protected: RwLock::new(false),
        };
        match vault.read_key_file()? {
            None => {
                let mut key = Zeroizing::new([0u8; KEY_LEN]);
                random_bytes(key.as_mut());
                vault.write_plain(&key)?;
                *vault.key.write() = Some(key);
            }
            Some(file) if file.mode == "plain" => {
                let encoded = file.key.ok_or_else(|| corrupt("the key is missing"))?;
                *vault.key.write() = Some(decode_key(&encoded)?);
            }
            Some(file) if file.mode == "password" => *vault.protected.write() = true,
            Some(file) => return Err(corrupt(&format!("unknown mode `{}`", file.mode))),
        }
        Ok(vault)
    }

    pub fn state(&self) -> VaultState {
        match (self.key.read().is_some(), *self.protected.read()) {
            (false, _) => VaultState::Locked,
            (true, true) => VaultState::Unlocked,
            (true, false) => VaultState::Open,
        }
    }

    /// Unlock a password-protected vault.
    pub fn unlock(&self, password: &str) -> AppResult<()> {
        let file = self
            .read_key_file()?
            .ok_or_else(|| corrupt("the key file has disappeared"))?;
        if file.mode != "password" {
            return Ok(());
        }
        let key = unwrap_key(&file, password)?;
        *self.key.write() = Some(key);
        Ok(())
    }

    /// Forget the key until [`unlock`](Self::unlock) is called again. No-op
    /// for a vault without a master password (it could not be reopened).
    pub fn lock(&self) {
        if *self.protected.read() {
            *self.key.write() = None;
        }
    }

    /// Set, change or (with `new: None`) remove the master password.
    /// The vault must be usable, and `current` must match when one is set.
    pub fn set_password(&self, current: Option<&str>, new: Option<&str>) -> AppResult<()> {
        if *self.protected.read() {
            let file = self
                .read_key_file()?
                .ok_or_else(|| corrupt("the key file has disappeared"))?;
            let current = current
                .ok_or_else(|| AppError::unauthorized("the current master password is required"))?;
            unwrap_key(&file, current)?;
        }
        let key = self.key.read().clone().ok_or_else(locked)?;
        match new {
            Some(password) => {
                if password.chars().count() < 8 {
                    return Err(AppError::invalid(
                        "the master password must be at least 8 characters long",
                    ));
                }
                let mut salt = [0u8; 16];
                random_bytes(&mut salt);
                let kek = derive(password, &salt)?;
                let mut nonce = [0u8; NONCE_LEN];
                random_bytes(&mut nonce);
                let cipher = XChaCha20Poly1305::new_from_slice(kek.as_ref())
                    .map_err(|_| AppError::internal("bad key length"))?;
                let ciphertext = cipher
                    .encrypt(
                        &XNonce::from(nonce),
                        Payload {
                            msg: key.as_ref(),
                            aad: b"nocturne-vault-key",
                        },
                    )
                    .map_err(|_| AppError::internal("could not protect the vault key"))?;
                let mut wrapped = nonce.to_vec();
                wrapped.extend(ciphertext);
                self.write_key_file(&KeyFile {
                    version: 1,
                    mode: "password".into(),
                    key: None,
                    salt: Some(BASE64.encode(salt)),
                    wrapped: Some(BASE64.encode(wrapped)),
                })?;
                *self.protected.write() = true;
            }
            None => {
                self.write_plain(&key)?;
                *self.protected.write() = false;
            }
        }
        Ok(())
    }

    /// Store a secret (replacing any previous value under the same id).
    pub fn put(&self, id: &str, secret: &str) -> AppResult<()> {
        let cipher = self.cipher()?;
        let mut nonce = [0u8; NONCE_LEN];
        random_bytes(&mut nonce);
        let ciphertext = cipher
            .encrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: secret.as_bytes(),
                    aad: id.as_bytes(),
                },
            )
            .map_err(|_| AppError::internal("could not encrypt the secret"))?;
        self.store.secret_put(id, &nonce, &ciphertext)
    }

    pub fn get(&self, id: &str) -> AppResult<Option<Zeroizing<String>>> {
        let Some((nonce, ciphertext)) = self.store.secret_get(id)? else {
            return Ok(None);
        };
        let cipher = self.cipher()?;
        let nonce: [u8; NONCE_LEN] = nonce.try_into().map_err(|_| corrupt_secret(id))?;
        let plaintext = cipher
            .decrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: &ciphertext,
                    aad: id.as_bytes(),
                },
            )
            .map_err(|_| corrupt_secret(id))?;
        String::from_utf8(plaintext)
            .map(|text| Some(Zeroizing::new(text)))
            .map_err(|_| corrupt_secret(id))
    }

    pub fn exists(&self, id: &str) -> AppResult<bool> {
        Ok(self.store.secret_get(id)?.is_some())
    }

    pub fn delete(&self, id: &str) -> AppResult<()> {
        self.store.secret_delete(id)
    }

    /// Delete every secret whose id starts with `prefix` (e.g. all parts of one credential).
    pub fn delete_prefix(&self, prefix: &str) -> AppResult<()> {
        for id in self.store.secret_ids(prefix)? {
            self.store.secret_delete(&id)?;
        }
        Ok(())
    }

    fn cipher(&self) -> AppResult<XChaCha20Poly1305> {
        let guard = self.key.read();
        let key = guard.as_ref().ok_or_else(locked)?;
        XChaCha20Poly1305::new_from_slice(key.as_ref())
            .map_err(|_| AppError::internal("bad key length"))
    }

    fn read_key_file(&self) -> AppResult<Option<KeyFile>> {
        let Some(path) = &self.key_path else {
            return Ok(None);
        };
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text)
                .map(Some)
                .map_err(|err| corrupt(&err.to_string())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    fn write_plain(&self, key: &[u8; KEY_LEN]) -> AppResult<()> {
        self.write_key_file(&KeyFile {
            version: 1,
            mode: "plain".into(),
            key: Some(BASE64.encode(key)),
            salt: None,
            wrapped: None,
        })
    }

    fn write_key_file(&self, file: &KeyFile) -> AppResult<()> {
        let Some(path) = &self.key_path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Write-then-rename so a crash cannot leave a half-written key file.
        let temporary = path.with_extension("key.tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(file)?)?;
        restrict_permissions(&temporary);
        std::fs::rename(&temporary, path)?;
        Ok(())
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {
    // Windows: the file inherits the per-user ACL of the application data directory.
}

fn derive(password: &str, salt: &[u8]) -> AppResult<Zeroizing<[u8; KEY_LEN]>> {
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    Argon2::default()
        .hash_password_into(password.as_bytes(), salt, key.as_mut())
        .map_err(|err| AppError::internal(format!("key derivation failed: {err}")))?;
    Ok(key)
}

fn unwrap_key(file: &KeyFile, password: &str) -> AppResult<Zeroizing<[u8; KEY_LEN]>> {
    let salt = BASE64
        .decode(file.salt.as_deref().unwrap_or_default())
        .map_err(|_| corrupt("bad salt"))?;
    let wrapped = BASE64
        .decode(file.wrapped.as_deref().unwrap_or_default())
        .map_err(|_| corrupt("bad wrapped key"))?;
    if wrapped.len() <= NONCE_LEN {
        return Err(corrupt("the wrapped key is truncated"));
    }
    let (nonce, ciphertext) = wrapped.split_at(NONCE_LEN);
    let nonce: [u8; NONCE_LEN] = nonce.try_into().map_err(|_| corrupt("bad nonce"))?;
    let kek = derive(password, &salt)?;
    let cipher = XChaCha20Poly1305::new_from_slice(kek.as_ref())
        .map_err(|_| AppError::internal("bad key length"))?;
    let key = cipher
        .decrypt(
            &XNonce::from(nonce),
            Payload {
                msg: ciphertext,
                aad: b"nocturne-vault-key",
            },
        )
        .map_err(|_| AppError::unauthorized("that is not the master password"))?;
    let key: [u8; KEY_LEN] = key
        .try_into()
        .map_err(|_| corrupt("the vault key has the wrong length"))?;
    Ok(Zeroizing::new(key))
}

fn decode_key(encoded: &str) -> AppResult<Zeroizing<[u8; KEY_LEN]>> {
    let bytes = BASE64
        .decode(encoded)
        .map_err(|_| corrupt("the key is not valid base64"))?;
    let key: [u8; KEY_LEN] = bytes
        .try_into()
        .map_err(|_| corrupt("the key has the wrong length"))?;
    Ok(Zeroizing::new(key))
}

fn locked() -> AppError {
    AppError::unauthorized("the credential vault is locked; enter the master password to continue")
}

fn corrupt(detail: &str) -> AppError {
    AppError::internal(format!(
        "the vault key file is damaged ({detail}); restore `vault.key` from a backup"
    ))
}

fn corrupt_secret(id: &str) -> AppError {
    AppError::internal(format!(
        "the stored secret `{id}` cannot be decrypted with this vault key (was the database copied without its `vault.key`?)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_vault() -> Vault {
        Vault::open(Arc::new(Store::in_memory().unwrap()), None).unwrap()
    }

    #[test]
    fn secrets_round_trip_and_are_not_stored_in_clear() {
        let store = Arc::new(Store::in_memory().unwrap());
        let vault = Vault::open(store.clone(), None).unwrap();
        vault.put("cred:1:password", "S3cret-Pa55").unwrap();
        assert_eq!(
            vault.get("cred:1:password").unwrap().unwrap().as_str(),
            "S3cret-Pa55"
        );
        assert!(vault.exists("cred:1:password").unwrap());
        assert!(vault.get("cred:1:missing").unwrap().is_none());

        let (_, ciphertext) = store.secret_get("cred:1:password").unwrap().unwrap();
        assert!(!ciphertext.windows(5).any(|window| window == b"S3cre"));
        // Same plaintext, fresh nonce: different ciphertext.
        vault.put("cred:2:password", "S3cret-Pa55").unwrap();
        assert_ne!(
            store.secret_get("cred:2:password").unwrap().unwrap().1,
            ciphertext
        );
    }

    #[test]
    fn ciphertext_is_bound_to_its_id() {
        let store = Arc::new(Store::in_memory().unwrap());
        let vault = Vault::open(store.clone(), None).unwrap();
        vault.put("cred:1:password", "alpha").unwrap();
        let (nonce, ciphertext) = store.secret_get("cred:1:password").unwrap().unwrap();
        store
            .secret_put("cred:2:password", &nonce, &ciphertext)
            .unwrap();
        assert!(
            vault.get("cred:2:password").is_err(),
            "a moved ciphertext must not decrypt"
        );
    }

    #[test]
    fn delete_by_prefix_removes_one_credential_only() {
        let vault = memory_vault();
        for id in [
            "cred:a:password",
            "cred:a:enable",
            "cred:ab:password",
            "cred:b:password",
        ] {
            vault.put(id, "x").unwrap();
        }
        vault.delete_prefix("cred:a:").unwrap();
        assert!(
            !vault.exists("cred:a:password").unwrap() && !vault.exists("cred:a:enable").unwrap()
        );
        assert!(
            vault.exists("cred:ab:password").unwrap() && vault.exists("cred:b:password").unwrap()
        );
    }

    #[test]
    fn key_file_persists_and_master_password_locks() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("n.db");
        let key_path = dir.path().join("vault.key");
        {
            let vault = Vault::open(Arc::new(Store::open(&db).unwrap()), Some(&key_path)).unwrap();
            assert_eq!(vault.state(), VaultState::Open);
            vault.put("s", "kept across restarts").unwrap();
            assert_eq!(
                vault.set_password(None, Some("short")).unwrap_err().code,
                "invalid"
            );
            vault
                .set_password(None, Some("correct horse battery"))
                .unwrap();
            assert_eq!(vault.state(), VaultState::Unlocked);
            assert!(
                !std::fs::read_to_string(&key_path)
                    .unwrap()
                    .contains("\"key\""),
                "the bare key must be gone from disk"
            );
        }
        let vault = Vault::open(Arc::new(Store::open(&db).unwrap()), Some(&key_path)).unwrap();
        assert_eq!(vault.state(), VaultState::Locked);
        assert_eq!(vault.get("s").unwrap_err().code, "unauthorized");
        assert_eq!(vault.put("t", "x").unwrap_err().code, "unauthorized");
        assert_eq!(
            vault.unlock("wrong password").unwrap_err().code,
            "unauthorized"
        );
        vault.unlock("correct horse battery").unwrap();
        assert_eq!(
            vault.get("s").unwrap().unwrap().as_str(),
            "kept across restarts"
        );

        // Changing and removing the password needs the current one.
        assert_eq!(
            vault
                .set_password(Some("nope nope nope"), None)
                .unwrap_err()
                .code,
            "unauthorized"
        );
        vault
            .set_password(Some("correct horse battery"), None)
            .unwrap();
        assert_eq!(vault.state(), VaultState::Open);
        vault.lock();
        assert_eq!(
            vault.state(),
            VaultState::Open,
            "an unprotected vault cannot be locked"
        );
        let reopened = Vault::open(Arc::new(Store::open(&db).unwrap()), Some(&key_path)).unwrap();
        assert_eq!(
            reopened.get("s").unwrap().unwrap().as_str(),
            "kept across restarts"
        );
    }

    #[test]
    fn a_database_without_its_key_file_explains_itself() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("n.db");
        let key_path = dir.path().join("vault.key");
        Vault::open(Arc::new(Store::open(&db).unwrap()), Some(&key_path))
            .unwrap()
            .put("s", "value")
            .unwrap();
        std::fs::remove_file(&key_path).unwrap();
        let vault = Vault::open(Arc::new(Store::open(&db).unwrap()), Some(&key_path)).unwrap();
        let message = vault.get("s").unwrap_err().message;
        assert!(message.contains("vault.key"), "{message}");
    }
}

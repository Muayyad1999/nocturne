//! Stored logins.
//!
//! A credential is a named set of what a device might ask for: username,
//! password, enable secret, an SSH private key, SNMP access. The record in the
//! database holds only the non-secret parts and flags saying which secrets
//! exist; the secrets themselves go straight into the vault and are never
//! returned over RPC. Feature modules that need them call [`resolve`].

use serde::{Deserialize, Serialize};
use serde_json::json;
use zeroize::Zeroizing;

use crate::app::{App, AppBuilder, NoParams};
use crate::error::{AppError, AppResult};
use crate::model::collections::{CREDENTIALS, DEVICES};
use crate::model::{Credential, Device, SnmpAccess};
use crate::util::{new_id, now_ms};

/// What the UI sends to create or update a credential. For each secret:
/// absent = leave as is, empty string = remove, anything else = set.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct CredentialInput {
    pub name: Option<String>,
    pub username: Option<String>,
    pub notes: Option<String>,
    pub password: Option<String>,
    pub enable_secret: Option<String>,
    pub private_key: Option<String>,
    pub passphrase: Option<String>,
    pub snmp_version: Option<String>,
    pub snmp_community: Option<String>,
    pub snmp_user: Option<String>,
    pub snmp_auth_protocol: Option<String>,
    pub snmp_auth_password: Option<String>,
    pub snmp_priv_protocol: Option<String>,
    pub snmp_priv_password: Option<String>,
}

/// A credential with its secrets, for handing to a transport. Never serialised.
#[derive(Default)]
pub struct Resolved {
    pub username: String,
    pub password: Option<Zeroizing<String>>,
    pub enable_secret: Option<Zeroizing<String>>,
    pub private_key: Option<Zeroizing<String>>,
    pub passphrase: Option<Zeroizing<String>>,
    pub snmp: SnmpAccess,
    pub snmp_community: Option<Zeroizing<String>>,
    pub snmp_auth_password: Option<Zeroizing<String>>,
    pub snmp_priv_password: Option<Zeroizing<String>>,
}

impl std::fmt::Debug for Resolved {
    /// Secrets never reach logs or panic messages: only whether each is set.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resolved")
            .field("username", &self.username)
            .field("password", &self.password.is_some())
            .field("enable_secret", &self.enable_secret.is_some())
            .field("private_key", &self.private_key.is_some())
            .field("passphrase", &self.passphrase.is_some())
            .field("snmp", &self.snmp)
            .field("snmp_community", &self.snmp_community.is_some())
            .field("snmp_auth_password", &self.snmp_auth_password.is_some())
            .field("snmp_priv_password", &self.snmp_priv_password.is_some())
            .finish()
    }
}

const SECRET_PARTS: [&str; 7] = [
    "password",
    "enable",
    "key",
    "passphrase",
    "snmp_community",
    "snmp_auth",
    "snmp_priv",
];

fn secret_id(credential_id: &str, part: &str) -> String {
    format!("cred:{credential_id}:{part}")
}

/// Fetch a credential and decrypt its secrets. Fails if the vault is locked.
pub fn resolve(app: &App, credential_id: &str) -> AppResult<Resolved> {
    let credential: Credential = app
        .store()
        .require(CREDENTIALS, credential_id, "credential")?;
    let secret = |part: &str| app.vault().get(&secret_id(credential_id, part));
    Ok(Resolved {
        username: credential.username,
        password: secret("password")?,
        enable_secret: secret("enable")?,
        private_key: secret("key")?,
        passphrase: secret("passphrase")?,
        snmp_community: secret("snmp_community")?,
        snmp_auth_password: secret("snmp_auth")?,
        snmp_priv_password: secret("snmp_priv")?,
        snmp: credential.snmp,
    })
}

pub fn list(app: &App) -> AppResult<Vec<Credential>> {
    let mut credentials: Vec<Credential> = app.store().list(CREDENTIALS)?;
    credentials.sort_by_key(|credential| credential.name.to_lowercase());
    Ok(credentials)
}

pub fn create(app: &App, input: CredentialInput) -> AppResult<Credential> {
    let now = now_ms();
    let credential = Credential {
        id: new_id(),
        created_at: now,
        ..Credential::default()
    };
    apply(app, credential, input)
}

pub fn update(app: &App, id: &str, input: CredentialInput) -> AppResult<Credential> {
    let credential: Credential = app.store().require(CREDENTIALS, id, "credential")?;
    apply(app, credential, input)
}

fn apply(app: &App, mut credential: Credential, input: CredentialInput) -> AppResult<Credential> {
    if let Some(name) = input.name {
        credential.name = name.trim().to_string();
    }
    if credential.name.is_empty() {
        return Err(AppError::invalid("give the credential a name"));
    }
    let clash = app
        .store()
        .named(CREDENTIALS, &credential.name)?
        .is_some_and(|id| id != credential.id);
    if clash {
        return Err(AppError::conflict(format!(
            "a credential named `{}` already exists",
            credential.name
        )));
    }
    if let Some(username) = input.username {
        credential.username = username.trim().to_string();
    }
    if let Some(notes) = input.notes {
        credential.notes = notes;
    }
    if let Some(version) = input.snmp_version {
        credential.snmp.version = match version.trim().to_ascii_lowercase().as_str() {
            "" | "none" => String::new(),
            "1" | "v1" => "v1".into(),
            "2" | "2c" | "v2c" => "v2c".into(),
            "3" | "v3" => "v3".into(),
            other => {
                return Err(AppError::invalid(format!(
                    "`{other}` is not an SNMP version (use v1, v2c or v3)"
                )));
            }
        };
    }
    if let Some(user) = input.snmp_user {
        credential.snmp.user = user.trim().to_string();
    }
    if let Some(protocol) = input.snmp_auth_protocol {
        credential.snmp.auth_protocol = one_of(
            "SNMP authentication protocol",
            &protocol,
            &["", "md5", "sha1", "sha224", "sha256", "sha384", "sha512"],
        )?;
    }
    if let Some(protocol) = input.snmp_priv_protocol {
        credential.snmp.priv_protocol = one_of(
            "SNMP privacy protocol",
            &protocol,
            &["", "des", "aes128", "aes192", "aes256"],
        )?;
    }

    // Secrets: write the vault first so a locked vault fails before anything is stored.
    let id = credential.id.clone();
    let set = |part: &str, value: Option<String>, flag: &mut bool| -> AppResult<()> {
        match value {
            None => {}
            Some(value) if value.is_empty() => {
                app.vault().delete(&secret_id(&id, part))?;
                *flag = false;
            }
            Some(value) => {
                app.vault().put(&secret_id(&id, part), &value)?;
                *flag = true;
            }
        }
        Ok(())
    };
    set("password", input.password, &mut credential.has_password)?;
    set(
        "enable",
        input.enable_secret,
        &mut credential.has_enable_secret,
    )?;
    set(
        "key",
        input.private_key.map(|key| normalise_key(&key)),
        &mut credential.has_private_key,
    )?;
    set(
        "passphrase",
        input.passphrase,
        &mut credential.has_passphrase,
    )?;
    set(
        "snmp_community",
        input.snmp_community,
        &mut credential.snmp.has_community,
    )?;
    set(
        "snmp_auth",
        input.snmp_auth_password,
        &mut credential.snmp.has_auth_password,
    )?;
    set(
        "snmp_priv",
        input.snmp_priv_password,
        &mut credential.snmp.has_priv_password,
    )?;

    if credential.snmp.version == "v3" && credential.snmp.user.is_empty() {
        return Err(AppError::invalid("SNMPv3 needs a user name"));
    }
    if !credential.snmp.priv_protocol.is_empty() && credential.snmp.auth_protocol.is_empty() {
        return Err(AppError::invalid(
            "SNMPv3 privacy requires authentication to be configured as well",
        ));
    }

    credential.updated_at = now_ms();
    app.store().put(CREDENTIALS, &credential.id, &credential)?;
    app.audit(
        "credential.save",
        None,
        format!("Saved credential `{}`", credential.name),
        serde_json::Value::Null,
    );
    app.publish("credentials.changed", &json!({"id": credential.id}));
    Ok(credential)
}

/// Pasted keys often arrive with Windows line endings or stray indentation.
fn normalise_key(key: &str) -> String {
    if key.trim().is_empty() {
        return String::new();
    }
    let mut cleaned: String = key
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    cleaned.push('\n');
    cleaned
}

fn one_of(what: &str, value: &str, allowed: &[&str]) -> AppResult<String> {
    let value = value.trim().to_ascii_lowercase().replace('-', "");
    if allowed.contains(&value.as_str()) {
        Ok(value)
    } else {
        let options: Vec<&str> = allowed
            .iter()
            .copied()
            .filter(|option| !option.is_empty())
            .collect();
        Err(AppError::invalid(format!(
            "`{value}` is not a supported {what} (choose one of: {})",
            options.join(", ")
        )))
    }
}

pub fn delete(app: &App, id: &str) -> AppResult<()> {
    let credential: Credential = app.store().require(CREDENTIALS, id, "credential")?;
    let users: Vec<String> = app
        .store()
        .list::<Device>(DEVICES)?
        .into_iter()
        .filter(|device| {
            device.credential_id.as_deref() == Some(id)
                || device.monitor.snmp_credential_id.as_deref() == Some(id)
        })
        .map(|device| device.name)
        .collect();
    if !users.is_empty() {
        let shown = users.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
        let more = if users.len() > 5 {
            format!(" and {} more", users.len() - 5)
        } else {
            String::new()
        };
        return Err(AppError::conflict(format!(
            "`{}` is still used by {shown}{more}; assign another credential to those devices first",
            credential.name
        )));
    }
    for part in SECRET_PARTS {
        app.vault().delete(&secret_id(id, part))?;
    }
    app.store().delete(CREDENTIALS, id)?;
    app.audit(
        "credential.delete",
        None,
        format!("Deleted credential `{}`", credential.name),
        serde_json::Value::Null,
    );
    app.publish("credentials.changed", &json!({"id": id, "deleted": true}));
    Ok(())
}

#[derive(Deserialize)]
struct IdParams {
    id: String,
}

#[derive(Deserialize)]
struct UpdateParams {
    id: String,
    #[serde(flatten)]
    input: CredentialInput,
}

#[derive(Serialize)]
struct Deleted {
    deleted: bool,
}

pub fn init(builder: &mut AppBuilder) {
    builder
        .router
        .add("credentials.list", |app: App, _: NoParams| async move {
            list(&app)
        });
    builder.router.add(
        "credentials.create",
        |app: App, input: CredentialInput| async move { create(&app, input) },
    );
    builder.router.add(
        "credentials.update",
        |app: App, params: UpdateParams| async move { update(&app, &params.id, params.input) },
    );
    builder.router.add(
        "credentials.delete",
        |app: App, params: IdParams| async move {
            delete(&app, &params.id)?;
            Ok(Deleted { deleted: true })
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppConfig;
    use serde_json::{Value, json};

    fn app() -> App {
        let mut builder = AppBuilder::new(AppConfig::in_memory()).unwrap();
        init(&mut builder);
        builder.build()
    }

    #[tokio::test]
    async fn secrets_go_to_the_vault_and_never_come_back_over_rpc() {
        let app = app();
        let created = app
            .rpc(
                "credentials.create",
                json!({"name": "Campus admin", "username": "netadmin", "password": "Hunter2!", "enable_secret": "En4ble",
                       "snmp_version": "2c", "snmp_community": "c0mmunity"}),
            )
            .await
            .unwrap();
        let text = created.to_string();
        for secret in ["Hunter2!", "En4ble", "c0mmunity"] {
            assert!(
                !text.contains(secret),
                "secret leaked in RPC result: {text}"
            );
        }
        assert_eq!(created["has_password"], true);
        assert_eq!(created["has_enable_secret"], true);
        assert_eq!(created["has_private_key"], false);
        assert_eq!(created["snmp"]["version"], "v2c");
        assert_eq!(created["snmp"]["has_community"], true);

        let id = created["id"].as_str().unwrap();
        let resolved = resolve(&app, id).unwrap();
        assert_eq!(resolved.username, "netadmin");
        assert_eq!(resolved.password.unwrap().as_str(), "Hunter2!");
        assert_eq!(resolved.enable_secret.unwrap().as_str(), "En4ble");
        assert_eq!(resolved.snmp_community.unwrap().as_str(), "c0mmunity");
        assert!(
            app.rpc("credentials.list", Value::Null)
                .await
                .unwrap()
                .to_string()
                .contains("Campus admin")
        );
    }

    #[tokio::test]
    async fn update_keeps_removes_or_replaces_each_secret() {
        let app = app();
        let created = create(
            &app,
            CredentialInput {
                name: Some("c".into()),
                username: Some("u".into()),
                password: Some("first".into()),
                enable_secret: Some("en".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let updated = app
            .rpc("credentials.update", json!({"id": created.id, "username": "u2", "password": "second", "enable_secret": ""}))
            .await
            .unwrap();
        assert_eq!(updated["username"], "u2");
        assert_eq!(updated["has_enable_secret"], false);
        let resolved = resolve(&app, &created.id).unwrap();
        assert_eq!(resolved.password.unwrap().as_str(), "second");
        assert!(resolved.enable_secret.is_none());

        // Omitted secrets are left alone.
        app.rpc(
            "credentials.update",
            json!({"id": created.id, "notes": "rotated"}),
        )
        .await
        .unwrap();
        assert_eq!(
            resolve(&app, &created.id)
                .unwrap()
                .password
                .unwrap()
                .as_str(),
            "second"
        );
    }

    #[tokio::test]
    async fn validation_and_deletion_rules() {
        let app = app();
        assert_eq!(
            app.rpc("credentials.create", json!({"username": "x"}))
                .await
                .unwrap_err()
                .code,
            "invalid"
        );
        let first = app
            .rpc("credentials.create", json!({"name": "Shared"}))
            .await
            .unwrap();
        assert_eq!(
            app.rpc("credentials.create", json!({"name": "shared"}))
                .await
                .unwrap_err()
                .code,
            "conflict"
        );
        let bad_v3 = app
            .rpc(
                "credentials.create",
                json!({"name": "v3", "snmp_version": "v3"}),
            )
            .await
            .unwrap_err();
        assert!(bad_v3.message.contains("user name"));
        let bad_proto = app
            .rpc(
                "credentials.create",
                json!({"name": "p", "snmp_auth_protocol": "sha3"}),
            )
            .await
            .unwrap_err();
        assert!(bad_proto.message.contains("sha256"));
        let v3 = app
            .rpc(
                "credentials.create",
                json!({"name": "v3", "snmp_version": "3", "snmp_user": "monitor", "snmp_auth_protocol": "SHA-256",
                       "snmp_auth_password": "authpass1", "snmp_priv_protocol": "aes128", "snmp_priv_password": "privpass1"}),
            )
            .await
            .unwrap();
        assert_eq!(v3["snmp"]["auth_protocol"], "sha256");

        // A credential in use cannot be deleted.
        let id = first["id"].as_str().unwrap().to_string();
        let device = Device {
            id: "d1".into(),
            name: "CORE-1".into(),
            credential_id: Some(id.clone()),
            ..Device::default()
        };
        app.store().put(DEVICES, "d1", &device).unwrap();
        let refused = app
            .rpc("credentials.delete", json!({"id": id}))
            .await
            .unwrap_err();
        assert_eq!(refused.code, "conflict");
        assert!(refused.message.contains("CORE-1"));
        app.store().delete(DEVICES, "d1").unwrap();
        app.rpc("credentials.delete", json!({"id": id}))
            .await
            .unwrap();
        assert_eq!(resolve(&app, &id).unwrap_err().code, "not_found");
    }

    #[test]
    fn pasted_keys_are_normalised() {
        let pasted = "  -----BEGIN OPENSSH PRIVATE KEY-----\r\n  b3BlbnNzaA==\r\n\r\n  -----END OPENSSH PRIVATE KEY-----  ";
        assert_eq!(
            normalise_key(pasted),
            "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaA==\n-----END OPENSSH PRIVATE KEY-----\n"
        );
        assert_eq!(normalise_key("  \n "), "");
    }
}

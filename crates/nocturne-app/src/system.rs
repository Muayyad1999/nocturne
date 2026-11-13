//! Application-level RPC: what is running, the vault, and the audit trail.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::app::{App, AppBuilder, NoParams};
use crate::error::AppError;
use crate::store::{LogEntry, LogQuery};
use crate::util::now_ms;
use crate::vault::VaultState;

pub struct SystemState {
    started_at: i64,
}

#[derive(Serialize)]
pub struct AppInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub started_at: i64,
    /// Where the database and key file live; `None` for an in-memory instance.
    pub data_dir: Option<String>,
    pub vault: VaultState,
    pub os: &'static str,
    pub arch: &'static str,
    pub methods: usize,
}

pub fn info(app: &App) -> AppInfo {
    AppInfo {
        name: "Nocturne",
        version: env!("CARGO_PKG_VERSION"),
        started_at: app.state::<SystemState>().started_at,
        data_dir: app.data_dir().map(|dir| dir.display().to_string()),
        vault: app.vault().state(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        methods: app.methods().len(),
    }
}

#[derive(Serialize)]
struct VaultStatus {
    state: VaultState,
}

fn vault_status(app: &App) -> VaultStatus {
    VaultStatus {
        state: app.vault().state(),
    }
}

#[derive(Deserialize)]
struct UnlockParams {
    password: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct SetPasswordParams {
    /// Required when a master password is already set.
    current: Option<String>,
    /// `None` or empty removes the master password.
    new: Option<String>,
}

/// Preferences of the application as a whole (the UI's look lives here too, so it follows the user's data).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralSettings {
    /// `night`, `day` or `system`.
    pub theme: String,
    /// `compact` or `comfortable`.
    pub density: String,
    /// `24h` or `12h`.
    pub time_format: String,
    /// Desktop app: closing the window hides it to the tray; monitoring, receivers and schedules keep running.
    pub keep_running_in_tray: bool,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        GeneralSettings {
            theme: "night".into(),
            density: "compact".into(),
            time_format: "24h".into(),
            keep_running_in_tray: false,
        }
    }
}

pub fn init(builder: &mut AppBuilder) {
    builder.state(SystemState {
        started_at: now_ms(),
    });
    crate::settings::declare(builder, "general", &GeneralSettings::default());
    let router = &mut builder.router;
    router.add(
        "app.info",
        |app: App, _: NoParams| async move { Ok(info(&app)) },
    );
    router.add("app.methods", |app: App, _: NoParams| async move {
        Ok(app.methods())
    });

    router.add("vault.state", |app: App, _: NoParams| async move {
        Ok(vault_status(&app))
    });
    router.add(
        "vault.unlock",
        |app: App, params: UnlockParams| async move {
            app.vault().unlock(&params.password)?;
            app.publish("vault.changed", &vault_status(&app));
            Ok(vault_status(&app))
        },
    );
    router.add("vault.lock", |app: App, _: NoParams| async move {
        if app.vault().state() == VaultState::Open {
            return Err(AppError::invalid(
                "set a master password first; without one there is nothing to lock",
            ));
        }
        app.vault().lock();
        app.audit(
            "vault.lock",
            None,
            "Locked the credential vault",
            serde_json::Value::Null,
        );
        app.publish("vault.changed", &vault_status(&app));
        Ok(vault_status(&app))
    });
    router.add(
        "vault.set_password",
        |app: App, params: SetPasswordParams| async move {
            let new = params
                .new
                .as_deref()
                .filter(|password| !password.is_empty());
            if let Some(password) = new
                && password.chars().count() < 8
            {
                return Err(AppError::invalid(
                    "the master password needs at least 8 characters",
                ));
            }
            app.vault().set_password(params.current.as_deref(), new)?;
            let message = if new.is_some() {
                "Set the master password"
            } else {
                "Removed the master password"
            };
            app.audit("vault.set_password", None, message, serde_json::Value::Null);
            app.publish("vault.changed", &vault_status(&app));
            Ok(vault_status(&app))
        },
    );

    // The audit trail is written by `App::audit`; this is how it is read.
    router.add("audit.list", |app: App, query: LogQuery| async move {
        let query = LogQuery {
            stream: Some("audit".into()),
            limit: Some(query.limit.unwrap_or(200).min(1000)),
            ..query
        };
        let entries: Vec<LogEntry> = app.store().log_query(&query)?;
        Ok(json!({"entries": entries}))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppConfig;
    use serde_json::{Value, json};

    #[tokio::test]
    async fn info_vault_and_audit() {
        let dir = tempfile::tempdir().unwrap();
        let mut builder = AppBuilder::new(AppConfig::at(dir.path())).unwrap();
        crate::settings::init(&mut builder);
        init(&mut builder);
        let app = builder.build();
        assert_eq!(
            app.rpc("settings.get", json!({"section": "general"}))
                .await
                .unwrap()["theme"],
            "night"
        );

        let info = app.rpc("app.info", Value::Null).await.unwrap();
        assert_eq!(
            (info["name"].as_str(), info["vault"].as_str()),
            (Some("Nocturne"), Some("open"))
        );
        assert!(info["methods"].as_u64().unwrap() >= 7);
        assert!(
            app.rpc("app.methods", Value::Null)
                .await
                .unwrap()
                .as_array()
                .unwrap()
                .contains(&json!("vault.unlock"))
        );

        // Nothing to lock until a master password exists; weak passwords are refused.
        assert_eq!(
            app.rpc("vault.lock", Value::Null).await.unwrap_err().code,
            "invalid"
        );
        assert_eq!(
            app.rpc("vault.set_password", json!({"new": "short"}))
                .await
                .unwrap_err()
                .code,
            "invalid"
        );
        let set = app
            .rpc("vault.set_password", json!({"new": "correct horse"}))
            .await
            .unwrap();
        assert_eq!(set["state"], "unlocked");
        assert_eq!(
            app.rpc("vault.lock", Value::Null).await.unwrap()["state"],
            "locked"
        );
        assert!(
            app.rpc("vault.unlock", json!({"password": "wrong"}))
                .await
                .is_err()
        );
        assert_eq!(
            app.rpc("vault.unlock", json!({"password": "correct horse"}))
                .await
                .unwrap()["state"],
            "unlocked"
        );
        // Removing it needs the current one.
        assert!(
            app.rpc("vault.set_password", json!({"new": ""}))
                .await
                .is_err()
        );
        assert_eq!(
            app.rpc("vault.set_password", json!({"current": "correct horse"}))
                .await
                .unwrap()["state"],
            "open"
        );

        let audit = app
            .rpc("audit.list", json!({"text": "master password"}))
            .await
            .unwrap();
        let messages: Vec<&str> = audit["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["message"].as_str().unwrap())
            .collect();
        assert_eq!(
            messages,
            ["Removed the master password", "Set the master password"]
        );
    }
}

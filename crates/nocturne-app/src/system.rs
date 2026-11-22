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
    /// Whether the caller is at the machine the engine runs on. False from
    /// `nocturne-server`, where the local shell, the serial ports and file
    /// transfer refer to a machine the person is not sitting at, so an
    /// interface can hide those rather than offer them and then fail.
    pub local_access: bool,
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
        local_access: app.local_access(),
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

/// Record that a password was tried on the vault and was wrong. The entry
/// says through which method, and never what was tried: a wrong password is
/// very often the right one for something else.
fn audit_wrong_password(app: &App, method: &str) {
    app.audit(
        "vault.wrong_password",
        None,
        "Refused a wrong password for the credential vault",
        json!({ "method": method }),
    );
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
            if let Err(err) = app.vault().unlock(&params.password) {
                // The one refusal `unlock` gives with this code is a password
                // that does not open the key file; a damaged or missing file
                // is `internal`.
                if err.code == "unauthorized" {
                    audit_wrong_password(&app, "vault.unlock");
                }
                return Err(err);
            }
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
            let before = app.vault().state();
            // A locked vault cannot have its password changed whatever is
            // typed, so it is refused before the current password is looked
            // at. Otherwise this method would say whether a guess was right
            // without the guess ever being recorded: `vault.unlock` is the
            // one place a locked vault tests a password, and it is audited.
            if before == VaultState::Locked {
                return Err(AppError::unauthorized(
                    "the credential vault is locked; unlock it before changing the master password",
                ));
            }
            if let Err(err) = app.vault().set_password(params.current.as_deref(), new) {
                // Unlocked and given a current password, the only thing left
                // to refuse with this code is that password being wrong (a
                // missing one is not a guess).
                if err.code == "unauthorized"
                    && before == VaultState::Unlocked
                    && params.current.is_some()
                {
                    audit_wrong_password(&app, "vault.set_password");
                }
                return Err(err);
            }
            let message = if new.is_some() {
                "Set the master password"
            } else {
                "Removed the master password"
            };
            // What happened, for whoever reads the trail by machine: the
            // sentence says "set" for a first password and a new one alike.
            let change = match (before, new.is_some()) {
                (VaultState::Open, true) => "set",
                (VaultState::Open, false) => "unchanged",
                (_, true) => "changed",
                (_, false) => "removed",
            };
            app.audit(
                "vault.set_password",
                None,
                message,
                json!({ "change": change }),
            );
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
        // An interface asks once, here, instead of probing a method that
        // needs local access and reading the refusal.
        assert_eq!(info["local_access"], json!(true));

        // And from a server it is told the truth, so it can hide the local
        // shell and the serial ports instead of offering a machine the
        // person is not sitting at.
        let mut remote = AppBuilder::new(AppConfig::in_memory().remote_users()).unwrap();
        crate::settings::init(&mut remote);
        init(&mut remote);
        let remote = remote.build();
        let info = remote.rpc("app.info", Value::Null).await.unwrap();
        assert_eq!(info["local_access"], json!(false));
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

    async fn audited(app: &App, action: &str) -> Vec<Value> {
        let audit = app
            .rpc("audit.list", json!({"text": action}))
            .await
            .unwrap();
        audit["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["source"] == action)
            .cloned()
            .collect()
    }

    /// An application on a directory: a master password lives in the key
    /// file, and an in-memory vault has none. The directory is returned so
    /// that it outlives the application.
    fn app_with_vault() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let mut builder = AppBuilder::new(AppConfig::at(dir.path())).unwrap();
        crate::settings::init(&mut builder);
        init(&mut builder);
        (dir, builder.build())
    }

    #[tokio::test]
    async fn a_wrong_vault_password_is_audited_and_what_was_tried_is_not() {
        let (_dir, app) = app_with_vault();
        app.rpc("vault.set_password", json!({"new": "correct horse"}))
            .await
            .unwrap();
        app.rpc("vault.lock", Value::Null).await.unwrap();

        for guess in ["hunter2-guess", "another-guess-9"] {
            let refused = app
                .rpc("vault.unlock", json!({"password": guess}))
                .await
                .unwrap_err();
            assert_eq!(refused.code, "unauthorized");
        }
        let wrong = audited(&app, "vault.wrong_password").await;
        assert_eq!(wrong.len(), 2, "{wrong:#?}");
        assert_eq!(wrong[0]["body"], json!({"method": "vault.unlock"}));
        assert_eq!(
            wrong[0]["message"],
            "Refused a wrong password for the credential vault"
        );

        // The right one is not a refusal.
        app.rpc("vault.unlock", json!({"password": "correct horse"}))
            .await
            .unwrap();
        assert_eq!(audited(&app, "vault.wrong_password").await.len(), 2);

        // Changing the password tests the current one too.
        let refused = app
            .rpc(
                "vault.set_password",
                json!({"current": "third-guess-77", "new": "something else"}),
            )
            .await
            .unwrap_err();
        assert_eq!(refused.code, "unauthorized");
        let wrong = audited(&app, "vault.wrong_password").await;
        assert_eq!(wrong.len(), 3, "{wrong:#?}");
        assert_eq!(wrong[0]["body"], json!({"method": "vault.set_password"}));

        // Leaving the current password out is a mistake, not a guess.
        app.rpc("vault.set_password", json!({"new": "something else"}))
            .await
            .unwrap_err();
        assert_eq!(audited(&app, "vault.wrong_password").await.len(), 3);

        // Nothing that was tried is anywhere in the trail.
        let everything = app
            .rpc("audit.list", Value::Null)
            .await
            .unwrap()
            .to_string();
        for tried in [
            "hunter2-guess",
            "another-guess-9",
            "third-guess-77",
            "correct horse",
            "something else",
        ] {
            assert!(!everything.contains(tried), "{tried} is in {everything}");
        }
    }

    #[tokio::test]
    async fn a_locked_vault_does_not_say_whether_a_password_is_right_except_through_unlock() {
        let (_dir, app) = app_with_vault();
        app.rpc("vault.set_password", json!({"new": "correct horse"}))
            .await
            .unwrap();
        app.rpc("vault.lock", Value::Null).await.unwrap();

        // Right or wrong, the answer is the same, so there is nothing to
        // learn here and nothing to record.
        let mut answers = Vec::new();
        for current in ["correct horse", "a wrong guess"] {
            let refused = app
                .rpc(
                    "vault.set_password",
                    json!({"current": current, "new": "something else"}),
                )
                .await
                .unwrap_err();
            assert_eq!(refused.code, "unauthorized");
            answers.push(refused.message);
        }
        assert_eq!(answers[0], answers[1]);
        assert!(answers[0].contains("locked"), "{}", answers[0]);
        assert!(audited(&app, "vault.wrong_password").await.is_empty());
        // And the password is still the one it was.
        assert_eq!(
            app.rpc("vault.unlock", json!({"password": "correct horse"}))
                .await
                .unwrap()["state"],
            "unlocked"
        );
    }

    #[tokio::test]
    async fn the_audit_entry_says_whether_the_master_password_was_set_changed_or_removed() {
        let (_dir, app) = app_with_vault();
        app.rpc("vault.set_password", json!({"new": "correct horse"}))
            .await
            .unwrap();
        app.rpc(
            "vault.set_password",
            json!({"current": "correct horse", "new": "battery staple"}),
        )
        .await
        .unwrap();
        app.rpc("vault.set_password", json!({"current": "battery staple"}))
            .await
            .unwrap();
        // There is none left to remove: recorded, but not as a removal.
        app.rpc("vault.set_password", json!({})).await.unwrap();

        let changes: Vec<Value> = audited(&app, "vault.set_password")
            .await
            .iter()
            .rev()
            .map(|entry| entry["body"]["change"].clone())
            .collect();
        assert_eq!(changes, ["set", "changed", "removed", "unchanged"]);
    }
}

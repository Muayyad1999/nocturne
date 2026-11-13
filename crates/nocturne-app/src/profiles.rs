//! Device-type profiles at run time.
//!
//! The built-in pack ships inside the binary; profiles a user writes (or the
//! assistant drafts) are stored in the database and layered on top. A user
//! profile with the id of a built-in replaces it, and deleting it brings the
//! built-in back.

use std::sync::Arc;

use nocturne_drivers::expr::Expressions;
use nocturne_drivers::profile::{Origin, Platform, Profile, ProfileSummary, Registry};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::app::{App, AppBuilder, NoParams};
use crate::error::{AppError, AppResult};
use crate::model::Device;
use crate::model::collections::DEVICES;
use crate::store::Store;
use crate::util::now_ms;

const COLLECTION: &str = "profiles";

/// A user profile as stored.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct StoredProfile {
    id: String,
    yaml: String,
    updated_at: i64,
}

/// A profile that could not be loaded, and why.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProfileProblem {
    pub id: String,
    pub origin: Origin,
    pub problems: Vec<String>,
}

pub struct ProfilesState {
    registry: RwLock<Arc<Registry>>,
    problems: RwLock<Vec<ProfileProblem>>,
    /// Profiles shipped by feature crates (see [`register`]).
    extras: RwLock<Vec<String>>,
}

/// Built-ins first, then user profiles (parents before children).
fn build(store: &Store, extras: &[String]) -> AppResult<(Registry, Vec<ProfileProblem>)> {
    let (mut registry, errors) = Registry::builtin();
    let mut problems: Vec<ProfileProblem> = errors
        .into_iter()
        .map(|err| ProfileProblem {
            id: err.profile,
            origin: Origin::Builtin,
            problems: err.problems,
        })
        .collect();
    // Profiles that feature crates ship count as built-in.
    for yaml in extras {
        if let Err(err) = registry.add(yaml, Origin::Builtin) {
            problems.push(ProfileProblem {
                id: err.profile,
                origin: Origin::Builtin,
                problems: err.problems,
            });
        }
    }

    let mut pending: Vec<StoredProfile> = store.list(COLLECTION)?;
    pending.sort_by(|a, b| a.id.cmp(&b.id));
    loop {
        let before = pending.len();
        let mut waiting = Vec::new();
        let mut failed = Vec::new();
        for stored in pending {
            match registry.add(&stored.yaml, Origin::User) {
                Ok(_) => {}
                Err(err)
                    if err
                        .problems
                        .iter()
                        .any(|problem| problem.starts_with("extends:")) =>
                {
                    waiting.push(stored)
                }
                Err(err) => failed.push(ProfileProblem {
                    id: stored.id,
                    origin: Origin::User,
                    problems: err.problems,
                }),
            }
        }
        problems.extend(failed);
        pending = waiting;
        if pending.is_empty() {
            break;
        }
        if pending.len() == before {
            // Whatever is left has a missing or cyclic parent: report the real error.
            for stored in pending {
                if let Err(err) = registry.add(&stored.yaml, Origin::User) {
                    problems.push(ProfileProblem {
                        id: stored.id,
                        origin: Origin::User,
                        problems: err.problems,
                    });
                }
            }
            break;
        }
    }
    Ok((registry, problems))
}

fn rebuild(app: &App) -> AppResult<()> {
    let state = app.state::<ProfilesState>();
    let extras = state.extras.read().clone();
    let (registry, problems) = build(app.store(), &extras)?;
    *state.registry.write() = Arc::new(registry);
    *state.problems.write() = problems;
    Ok(())
}

/// The current registry. Cheap to call; hold it for as long as needed.
pub fn registry(app: &App) -> Arc<Registry> {
    app.state::<ProfilesState>().registry.read().clone()
}

pub fn expressions(app: &App) -> Arc<Expressions> {
    registry(app).expressions()
}

/// The compiled platform for a profile id.
pub fn platform(app: &App, id: &str) -> AppResult<Arc<Platform>> {
    registry(app).get(id).ok_or_else(|| {
        AppError::not_found(format!("device type `{id}`"))
            .with_details(json!({"hint": "Choose another device type for this device, or add a profile with this id under Settings → Device types."}))
    })
}

pub fn problems(app: &App) -> Vec<ProfileProblem> {
    app.state::<ProfilesState>().problems.read().clone()
}

/// Ship a profile with a feature crate (e.g. the lab's simulated host). Call
/// from the feature's `init`, after the core's. The profile counts as built-in:
/// users can override it but not delete it.
pub fn register(builder: &mut AppBuilder, yaml: &str) {
    let state = builder.get::<ProfilesState>();
    state.extras.write().push(yaml.to_string());
    let extras = state.extras.read().clone();
    match build(builder.store(), &extras) {
        Ok((registry, problems)) => {
            for problem in problems
                .iter()
                .filter(|problem| problem.origin == Origin::Builtin)
            {
                tracing::warn!(profile = %problem.id, problems = ?problem.problems, "device type could not be loaded");
            }
            *state.registry.write() = Arc::new(registry);
            *state.problems.write() = problems;
        }
        Err(err) => {
            tracing::error!(%err, "device types could not be rebuilt after registering a profile")
        }
    }
}

/// The profiles that exist without any user profile: the bundled pack and feature-registered ones.
fn shipped(app: &App) -> Registry {
    let extras = app.state::<ProfilesState>().extras.read().clone();
    let mut registry = Registry::builtin().0;
    for yaml in &extras {
        let _ = registry.add(yaml, Origin::Builtin);
    }
    registry
}

#[derive(Serialize)]
pub struct ProfileList {
    pub profiles: Vec<ProfileRow>,
    pub problems: Vec<ProfileProblem>,
}

#[derive(Serialize)]
pub struct ProfileRow {
    #[serde(flatten)]
    pub summary: ProfileSummary,
    /// A user profile that replaces a built-in of the same id.
    pub overrides_builtin: bool,
    /// Devices currently using this profile.
    pub devices: usize,
}

pub fn list(app: &App) -> AppResult<ProfileList> {
    let devices: Vec<Device> = app.store().list(DEVICES)?;
    let builtins = shipped(app);
    let profiles = registry(app)
        .list()
        .into_iter()
        .map(|summary| ProfileRow {
            overrides_builtin: summary.origin == Origin::User
                && builtins.get(&summary.id).is_some(),
            devices: devices
                .iter()
                .filter(|device| device.profile == summary.id)
                .count(),
            summary,
        })
        .collect();
    Ok(ProfileList {
        profiles,
        problems: problems(app),
    })
}

#[derive(Serialize)]
pub struct ProfileDetail {
    pub id: String,
    pub origin: Origin,
    /// The YAML as written (before `extends` is applied).
    pub yaml: String,
    /// The profile with inheritance applied.
    pub resolved: Profile,
    pub updated_at: Option<i64>,
}

pub fn get(app: &App, id: &str) -> AppResult<ProfileDetail> {
    let platform = platform(app, id)?;
    let stored: Option<StoredProfile> = app.store().get(COLLECTION, id)?;
    Ok(ProfileDetail {
        id: id.to_string(),
        origin: platform.origin,
        yaml: platform.source.clone(),
        resolved: platform.profile.clone(),
        updated_at: stored.map(|stored| stored.updated_at),
    })
}

#[derive(Serialize)]
pub struct Validation {
    pub ok: bool,
    pub id: String,
    pub problems: Vec<String>,
    /// The profile with inheritance applied, when it is valid.
    pub resolved: Option<Profile>,
}

/// Check a profile without saving it.
pub fn validate(app: &App, yaml: &str) -> AppResult<Validation> {
    // Validate against a scratch registry so a failed attempt cannot disturb the live one.
    let extras = app.state::<ProfilesState>().extras.read().clone();
    let (mut scratch, _) = build(app.store(), &extras)?;
    Ok(match scratch.add(yaml, Origin::User) {
        Ok(platform) => {
            let id = platform.profile.id.clone();
            let mut problems = Vec::new();
            if id.trim().is_empty() {
                problems.push(
                    "id: a profile needs an id (lower-case letters, digits and underscores)"
                        .to_string(),
                );
            } else if !id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            {
                problems.push(format!(
                    "id: `{id}` may only contain lower-case letters, digits and underscores"
                ));
            }
            Validation {
                ok: problems.is_empty(),
                id,
                resolved: problems.is_empty().then(|| platform.profile.clone()),
                problems,
            }
        }
        Err(err) => Validation {
            ok: false,
            id: err.profile,
            problems: err.problems,
            resolved: None,
        },
    })
}

/// Save a user profile (new, changed, or an override of a built-in).
pub fn save(app: &App, yaml: &str) -> AppResult<ProfileDetail> {
    let checked = validate(app, yaml)?;
    if !checked.ok {
        return Err(AppError::invalid(format!(
            "the profile has problems: {}",
            checked.problems.join("; ")
        ))
        .with_details(json!({"problems": checked.problems})));
    }
    let existed = app
        .store()
        .get::<StoredProfile>(COLLECTION, &checked.id)?
        .is_some();
    let stored = StoredProfile {
        id: checked.id.clone(),
        yaml: yaml.to_string(),
        updated_at: now_ms(),
    };
    app.store().put(COLLECTION, &stored.id, &stored)?;
    rebuild(app)?;
    app.audit(
        "profile.save",
        None,
        format!(
            "{} device type `{}`",
            if existed { "Updated" } else { "Added" },
            checked.id
        ),
        serde_json::Value::Null,
    );
    app.publish(
        "profiles.changed",
        &json!({"id": checked.id, "change": if existed { "updated" } else { "created" }}),
    );
    get(app, &checked.id)
}

#[derive(Debug, Serialize)]
pub struct Deleted {
    /// The built-in profile of the same id is active again.
    pub restored_builtin: bool,
    /// Devices that were using the profile and now have none.
    pub devices_affected: usize,
}

pub fn delete(app: &App, id: &str) -> AppResult<Deleted> {
    if app.store().get::<StoredProfile>(COLLECTION, id)?.is_none() {
        return Err(match registry(app).get(id) {
            Some(_) => AppError::invalid(format!(
                "`{id}` is a built-in device type and cannot be deleted"
            )),
            None => AppError::not_found(format!("device type `{id}`")),
        });
    }
    let devices: Vec<Device> = app.store().list(DEVICES)?;
    let children: Vec<String> = registry(app)
        .list()
        .into_iter()
        .filter(|summary| summary.extends.as_deref() == Some(id) && summary.origin == Origin::User)
        .map(|summary| summary.id)
        .collect();
    let restored_builtin = shipped(app).get(id).is_some();
    if !children.is_empty() && !restored_builtin {
        return Err(AppError::conflict(format!(
            "other device types extend `{id}` ({}); change or delete them first",
            children.join(", ")
        )));
    }
    app.store().delete(COLLECTION, id)?;
    rebuild(app)?;
    let devices_affected = if restored_builtin {
        0
    } else {
        devices.iter().filter(|device| device.profile == id).count()
    };
    app.audit(
        "profile.delete",
        None,
        format!("Deleted device type `{id}`"),
        serde_json::Value::Null,
    );
    app.publish("profiles.changed", &json!({"id": id, "change": "deleted"}));
    Ok(Deleted {
        restored_builtin,
        devices_affected,
    })
}

#[derive(Deserialize)]
struct IdParams {
    id: String,
}

#[derive(Deserialize)]
struct YamlParams {
    yaml: String,
}

pub fn init(builder: &mut AppBuilder) {
    let (registry, problems) = match build(builder.store(), &[]) {
        Ok(built) => built,
        Err(err) => {
            tracing::error!(%err, "user device types could not be read; only the built-in pack is loaded");
            let (registry, errors) = Registry::builtin();
            let problems = errors
                .into_iter()
                .map(|err| ProfileProblem {
                    id: err.profile,
                    origin: Origin::Builtin,
                    problems: err.problems,
                })
                .collect();
            (registry, problems)
        }
    };
    for problem in &problems {
        tracing::warn!(profile = %problem.id, problems = ?problem.problems, "device type could not be loaded");
    }
    builder.state(ProfilesState {
        registry: RwLock::new(Arc::new(registry)),
        problems: RwLock::new(problems),
        extras: RwLock::new(Vec::new()),
    });

    let router = &mut builder.router;
    router.add("profiles.list", |app: App, _: NoParams| async move {
        list(&app)
    });
    router.add("profiles.get", |app: App, params: IdParams| async move {
        get(&app, &params.id)
    });
    router.add(
        "profiles.validate",
        |app: App, params: YamlParams| async move { validate(&app, &params.yaml) },
    );
    router.add("profiles.save", |app: App, params: YamlParams| async move {
        save(&app, &params.yaml)
    });
    router.add("profiles.delete", |app: App, params: IdParams| async move {
        delete(&app, &params.id)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppConfig;
    use crate::testing;
    use serde_json::{Value, json};

    fn app() -> App {
        let mut builder = AppBuilder::new(AppConfig::in_memory()).unwrap();
        crate::credentials::init(&mut builder);
        crate::inventory::init(&mut builder);
        init(&mut builder);
        builder.build()
    }

    #[tokio::test]
    async fn user_profiles_are_validated_saved_listed_and_deleted() {
        let app = app();
        let mut events = app.subscribe();

        let bad = app
            .rpc(
                "profiles.validate",
                json!({"yaml": "id: Bad Id\ncli:\n  prompt: '(unclosed'\n"}),
            )
            .await
            .unwrap();
        assert_eq!(bad["ok"], false);
        assert!(!bad["problems"].as_array().unwrap().is_empty());
        assert_eq!(
            app.rpc("profiles.save", json!({"yaml": "id: x\nnonsense_key: 1\n"}))
                .await
                .unwrap_err()
                .code,
            "invalid"
        );
        // A rejected profile leaves nothing behind.
        assert_eq!(
            app.rpc("profiles.get", json!({"id": "x"}))
                .await
                .unwrap_err()
                .code,
            "not_found"
        );

        let ok = app
            .rpc("profiles.validate", json!({"yaml": testing::LAB_IOS}))
            .await
            .unwrap();
        assert_eq!(
            (ok["ok"].as_bool(), ok["id"].as_str()),
            (Some(true), Some("test_ios"))
        );
        assert_eq!(ok["resolved"]["config"]["syntax"], "ios");
        assert_eq!(
            app.rpc("profiles.get", json!({"id": "test_ios"}))
                .await
                .unwrap_err()
                .code,
            "not_found",
            "validate must not register"
        );

        let saved = app
            .rpc("profiles.save", json!({"yaml": testing::LAB_IOS}))
            .await
            .unwrap();
        assert_eq!(
            (saved["origin"].as_str(), saved["resolved"]["name"].as_str()),
            (Some("user"), Some("Test IOS"))
        );
        assert!(
            saved["yaml"]
                .as_str()
                .unwrap()
                .contains("terminal length 0")
        );
        assert!(
            platform(&app, "test_ios")
                .unwrap()
                .policy
                .is_read_only("show version")
        );

        // A child of a user profile, saved afterwards.
        let child = "id: test_ios_child\nextends: test_ios\nname: Child\n";
        app.rpc("profiles.save", json!({"yaml": child}))
            .await
            .unwrap();
        assert_eq!(
            platform(&app, "test_ios_child")
                .unwrap()
                .profile
                .config
                .running,
            "show running-config"
        );

        app.rpc(
            "devices.create",
            json!({"name": "sw1", "address": "10.0.0.1", "profile": "test_ios"}),
        )
        .await
        .unwrap();
        let listed = app.rpc("profiles.list", Value::Null).await.unwrap();
        let row = listed["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == "test_ios")
            .unwrap();
        assert_eq!(
            (
                row["devices"].as_u64(),
                row["overrides_builtin"].as_bool(),
                row["can_configure"].as_bool()
            ),
            (Some(1), Some(false), Some(true))
        );

        // The parent cannot go while a child extends it.
        assert_eq!(
            app.rpc("profiles.delete", json!({"id": "test_ios"}))
                .await
                .unwrap_err()
                .code,
            "conflict"
        );
        app.rpc("profiles.delete", json!({"id": "test_ios_child"}))
            .await
            .unwrap();
        let deleted = app
            .rpc("profiles.delete", json!({"id": "test_ios"}))
            .await
            .unwrap();
        assert_eq!(
            (
                deleted["restored_builtin"].as_bool(),
                deleted["devices_affected"].as_u64()
            ),
            (Some(false), Some(1))
        );
        assert_eq!(
            app.rpc("profiles.delete", json!({"id": "test_ios"}))
                .await
                .unwrap_err()
                .code,
            "not_found"
        );

        let mut changes = Vec::new();
        while let Ok(event) = events.try_recv() {
            if event.topic == "profiles.changed" {
                changes.push(event.payload["change"].as_str().unwrap().to_string());
            }
        }
        assert_eq!(changes, ["created", "created", "deleted", "deleted"]);
    }

    #[tokio::test]
    async fn user_profiles_survive_a_restart_in_dependency_order() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut builder = AppBuilder::new(AppConfig::at(dir.path())).unwrap();
            init(&mut builder);
            let app = builder.build();
            // Saved child-first alphabetically: `a_child` extends `z_parent`.
            save(
                &app,
                &testing::LAB_IOS.replace("id: test_ios", "id: z_parent"),
            )
            .unwrap();
            save(&app, "id: a_child\nextends: z_parent\nname: Child\n").unwrap();
        }
        let mut builder = AppBuilder::new(AppConfig::at(dir.path())).unwrap();
        init(&mut builder);
        let app = builder.build();
        assert_eq!(
            platform(&app, "a_child").unwrap().profile.cli.init,
            vec!["terminal length 0".to_string()]
        );
        assert!(
            problems(&app)
                .iter()
                .all(|problem| problem.origin == Origin::Builtin),
            "{:?}",
            problems(&app)
        );
    }

    #[tokio::test]
    async fn feature_crates_can_ship_profiles_that_users_override_but_cannot_delete() {
        let mut builder = AppBuilder::new(AppConfig::in_memory()).unwrap();
        init(&mut builder);
        register(
            &mut builder,
            &testing::LAB_IOS.replace("id: test_ios", "id: shipped_ios"),
        );
        let app = builder.build();
        assert_eq!(
            platform(&app, "shipped_ios").unwrap().origin,
            Origin::Builtin
        );
        assert_eq!(delete(&app, "shipped_ios").unwrap_err().code, "invalid");

        // A user override wins, survives a rebuild, and deleting it restores the shipped one.
        let custom = testing::LAB_IOS
            .replace("id: test_ios", "id: shipped_ios")
            .replace("name: Test IOS", "name: Mine");
        save(&app, &custom).unwrap();
        save(&app, testing::LAB_IOS).unwrap();
        assert_eq!(platform(&app, "shipped_ios").unwrap().profile.name, "Mine");
        let listed = list(&app).unwrap();
        assert!(
            listed
                .profiles
                .iter()
                .any(|row| row.summary.id == "shipped_ios" && row.overrides_builtin)
        );
        assert!(delete(&app, "shipped_ios").unwrap().restored_builtin);
        assert_eq!(
            platform(&app, "shipped_ios").unwrap().profile.name,
            "Test IOS"
        );
    }
}

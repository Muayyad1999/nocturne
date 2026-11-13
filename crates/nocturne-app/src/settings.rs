//! User settings: small JSON documents, one per section.
//!
//! A module declares its section with defaults at init time
//! ([`declare`]); reads merge what the user stored over those defaults, so a
//! setting added in a later version simply appears with its default value.

use std::collections::BTreeMap;

use parking_lot::RwLock;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::app::{App, AppBuilder, NoParams};
use crate::error::{AppError, AppResult};

#[derive(Default)]
pub struct SettingsState {
    defaults: RwLock<BTreeMap<String, Value>>,
}

/// Register a section and its default values (a JSON object).
///
/// Two modules may share a section — the change pipeline and the assistant
/// both have something to say about autonomy — so declaring is a merge, not a
/// replacement: keys a previous module declared are kept, and new ones are
/// added. Otherwise whichever module happened to start last would silently
/// decide the whole section, and settings nobody declared would read as
/// missing.
pub fn declare<T: Serialize>(builder: &mut AppBuilder, section: &str, defaults: &T) {
    let value = serde_json::to_value(defaults).unwrap_or(Value::Null);
    let state = builder.get::<SettingsState>();
    let mut defaults = state.defaults.write();
    match defaults.get_mut(section) {
        // `merge` lets the patch win; here the module already registered does,
        // so the arguments are the other way round.
        Some(existing) => *existing = merge(value, existing.clone()),
        None => {
            defaults.insert(section.to_string(), value);
        }
    }
}

/// Read a section as a typed value (stored values over defaults).
pub fn get<T: DeserializeOwned>(app: &App, section: &str) -> AppResult<T> {
    let merged = get_value(app, section)?;
    serde_json::from_value(merged).map_err(|err| {
        AppError::internal(format!(
            "settings section `{section}` cannot be read: {err}"
        ))
    })
}

pub fn get_value(app: &App, section: &str) -> AppResult<Value> {
    let state = app.state::<SettingsState>();
    let defaults = state
        .defaults
        .read()
        .get(section)
        .cloned()
        .ok_or_else(|| AppError::not_found(format!("settings section `{section}`")))?;
    let stored: Option<Value> = app.store().kv_get(&key(section))?;
    Ok(match stored {
        Some(stored) => merge(defaults, stored),
        None => defaults,
    })
}

/// Replace the given keys of a section (a partial update) and announce it.
pub fn set_value(app: &App, section: &str, patch: Value) -> AppResult<Value> {
    if !patch.is_object() {
        return Err(AppError::invalid(
            "settings are updated with a JSON object of the keys to change",
        ));
    }
    let state = app.state::<SettingsState>();
    if !state.defaults.read().contains_key(section) {
        return Err(AppError::not_found(format!("settings section `{section}`")));
    }
    let stored: Value = app
        .store()
        .kv_get(&key(section))?
        .unwrap_or_else(|| Value::Object(Default::default()));
    app.store().kv_set(&key(section), &merge(stored, patch))?;
    let merged = get_value(app, section)?;
    app.publish(
        "settings.changed",
        &serde_json::json!({"section": section, "value": merged}),
    );
    Ok(merged)
}

fn key(section: &str) -> String {
    format!("settings.{section}")
}

/// Objects merge key by key; anything else is replaced by the patch.
fn merge(base: Value, patch: Value) -> Value {
    match (base, patch) {
        (Value::Object(mut base), Value::Object(patch)) => {
            for (key, value) in patch {
                let merged = match base.remove(&key) {
                    Some(existing) => merge(existing, value),
                    None => value,
                };
                base.insert(key, merged);
            }
            Value::Object(base)
        }
        (_, patch) => patch,
    }
}

#[derive(Deserialize)]
struct SectionParams {
    section: String,
}

#[derive(Deserialize)]
struct SetParams {
    section: String,
    value: Value,
}

pub fn init(builder: &mut AppBuilder) {
    builder.state(SettingsState::default());
    builder.router.add(
        "settings.get",
        |app: App, params: SectionParams| async move { get_value(&app, &params.section) },
    );
    builder
        .router
        .add("settings.set", |app: App, params: SetParams| async move {
            set_value(&app, &params.section, params.value)
        });
    builder
        .router
        .add("settings.all", |app: App, _: NoParams| async move {
            let sections: Vec<String> = app
                .state::<SettingsState>()
                .defaults
                .read()
                .keys()
                .cloned()
                .collect();
            let mut all = serde_json::Map::new();
            for section in sections {
                all.insert(section.clone(), get_value(&app, &section)?);
            }
            Ok(Value::Object(all))
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppConfig;
    use serde_json::json;

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct General {
        theme: String,
        density: String,
        nested: Nested,
    }

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct Nested {
        a: u32,
        b: u32,
    }

    #[tokio::test]
    async fn two_modules_can_declare_one_section() {
        let mut builder = AppBuilder::new(AppConfig::in_memory()).unwrap();
        init(&mut builder);
        // What one module knows about the section...
        declare(&mut builder, "shared", &json!({"autonomy": "draft"}));
        // ...and what another knows, including its own view of a shared key.
        declare(
            &mut builder,
            "shared",
            &json!({"autonomy": "lab", "max_steps": 40}),
        );
        let app = builder.build();

        let value = get_value(&app, "shared").unwrap();
        assert_eq!(
            value["max_steps"], 40,
            "the later module's own keys are there"
        );
        assert_eq!(
            value["autonomy"], "draft",
            "a key already declared is not overwritten"
        );
    }

    #[tokio::test]
    async fn stored_values_overlay_defaults() {
        let mut builder = AppBuilder::new(AppConfig::in_memory()).unwrap();
        init(&mut builder);
        declare(
            &mut builder,
            "general",
            &General {
                theme: "night".into(),
                density: "compact".into(),
                nested: Nested { a: 1, b: 2 },
            },
        );
        let app = builder.build();

        let general: General = get(&app, "general").unwrap();
        assert_eq!(general.theme, "night");

        let mut events = app.subscribe();
        let updated = app
            .rpc(
                "settings.set",
                json!({"section": "general", "value": {"theme": "day", "nested": {"b": 9}}}),
            )
            .await
            .unwrap();
        assert_eq!(
            updated,
            json!({"theme": "day", "density": "compact", "nested": {"a": 1, "b": 9}})
        );
        assert_eq!(events.recv().await.unwrap().topic, "settings.changed");
        assert_eq!(
            get::<General>(&app, "general").unwrap().nested,
            Nested { a: 1, b: 9 }
        );

        assert_eq!(
            app.rpc("settings.get", json!({"section": "nope"}))
                .await
                .unwrap_err()
                .code,
            "not_found"
        );
        assert_eq!(
            app.rpc("settings.set", json!({"section": "general", "value": 3}))
                .await
                .unwrap_err()
                .code,
            "invalid"
        );
        assert!(app.rpc("settings.all", Value::Null).await.unwrap()["general"].is_object());
    }
}

//! The whole Nocturne backend, assembled.
//!
//! The desktop shell and the headless server both call [`start`] and then talk
//! to the returned [`App`] through `App::rpc` and `App::subscribe`; neither
//! knows which feature modules exist.

pub use nocturne_app::{App, AppBuilder, AppConfig, AppError, AppResult, Event};

/// Register the core and every feature module on a builder.
///
/// The order is the order the modules see each other in: the core first
/// (settings, credentials, inventory, jobs, device types, connections,
/// collected data), then the features, each of which only adds RPC methods,
/// events, settings and background tasks of its own.
pub fn init(builder: &mut AppBuilder) {
    nocturne_app::init(builder);
    nocturne_terminal::init(builder);
    nocturne_config::init(builder);
    nocturne_changes::init(builder);
    nocturne_lab::init(builder);
}

/// Assemble and start the application.
pub fn start(config: AppConfig) -> AppResult<App> {
    let mut builder = AppBuilder::new(config)?;
    init(&mut builder);
    Ok(builder.build())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every feature that is part of the build must be reachable, and no two
    /// may claim the same method name (the router panics if they do, so this
    /// test also proves the whole set can be assembled at all).
    #[tokio::test]
    async fn every_feature_registers_its_methods() {
        let app = start(AppConfig::in_memory()).unwrap();
        let methods = app.methods();
        for expected in [
            // core
            "app.info",
            "devices.list",
            "credentials.list",
            "jobs.list",
            "profiles.list",
            "settings.all",
            "vault.state",
            "collect.run",
            "devices.test",
            // terminal
            "terminal.open",
            "terminal.attach",
            "terminal.list",
            "sftp.open",
            "keys.generate",
            // configuration
            "config.backup",
            "config.search",
            "config.diff",
            "compliance.run",
            "config.drift",
            // changes
            "changes.create",
            "changes.plan",
            "changes.approve",
            "changes.apply",
            "changes.rollback",
            "changes.assistant_apply",
            "changes.preview",
            // the lab
            "labs.create",
            "labs.start",
            "labs.node.add",
            "labs.link.add",
            "labs.connectivity",
            "labs.expose",
            "labs.twin",
            "config.drift",
        ] {
            assert!(
                methods.contains(&expected),
                "`{expected}` is missing from {} methods",
                methods.len()
            );
        }
        let mut sorted = methods.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            methods.len(),
            "two modules registered the same method"
        );
    }

    #[tokio::test]
    async fn starts_and_answers_rpc() {
        let app = start(AppConfig::in_memory()).unwrap();
        let devices = app
            .rpc("devices.list", serde_json::Value::Null)
            .await
            .unwrap();
        assert_eq!(devices, serde_json::json!([]));
        assert!(app.methods().contains(&"settings.all"));
    }
}

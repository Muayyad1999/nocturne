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
    nocturne_discovery::init(builder);
    nocturne_terminal::init(builder);
    nocturne_config::init(builder);
    nocturne_changes::init(builder);
    nocturne_lab::init(builder);
    nocturne_logs::init(builder);
    nocturne_automation::init(builder);
    nocturne_topology::init(builder);
    nocturne_monitor::init(builder);
    // After monitoring and the topology, which it asks for alerts, interface
    // rates and links when they are there, and does without when they are not.
    nocturne_insights::init(builder);
    // After the change engine, deliberately: both declare the `assistant`
    // settings section — they share `autonomy` — and a later `declare` wins
    // for the keys it names. The assistant declares all nine, the change
    // engine only `autonomy`, so this order is the one where every key is
    // visible to a settings screen.
    nocturne_assistant::init(builder);
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
            // monitoring
            "monitor.status",
            "monitor.poll_now",
            "metrics.query",
            "metrics.top",
            "alerts.list",
            "alerts.ack",
            "alerts.rules.save",
            "maintenance.save",
            "notify.channels.save",
            // discovery
            "discovery.suggest",
            "discovery.scan",
            "discovery.scans",
            "discovery.preview",
            "discovery.add",
            "discovery.crawl",
            "discovery.ignore",
            // logs
            "logs.query",
            "logs.histogram",
            "logs.follow",
            "logs.pull",
            "logs.rules.save",
            "logs.status",
            // automation
            "tasks.list",
            "tasks.run",
            "runner.run",
            "runner.compare",
            "templates.list",
            "templates.render",
            "playbooks.run",
            "schedules.save",
            "triggers.save",
            "ansible.status",
            // the topology
            "topology.get",
            "topology.path",
            "topology.neighbors",
            "topology.export",
            "locate.host",
            // the network designer
            "design.examples",
            "design.validate",
            "design.generate",
            "design.create_lab",
            // insights
            "insights.scan",
            "insights.list",
            "insights.checks",
            "insights.coverage",
            "insights.dismiss",
            "insights.fix",
            // the assistant
            "ai.status",
            "ai.send",
            "ai.cancel",
            "ai.approve",
            "ai.chats.list",
            "ai.providers.list",
            "ai.providers.test",
            "ai.tools",
            "ai.usage",
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

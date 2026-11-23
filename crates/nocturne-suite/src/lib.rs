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
    nocturne_ipam::init(builder);
    nocturne_toolbox::init(builder);
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
    // Last, because it reads every other feature: what is down, what is
    // firing, what changed, what is drifting. It copes with any of them being
    // absent, but registering it here means a fresh build's overview says
    // "that feature is not installed" rather than nothing at all.
    nocturne_ops::init(builder);
    // The ISP edition's one feature, and the only conditional registration in
    // this function. It is last because nothing in the free set may depend on
    // it: every other module above must assemble, and its tests pass, with
    // this line compiled out.
    #[cfg(feature = "isp")]
    nocturne_isp::init(builder);
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
            "terminal.snippets.render",
            "terminal.snippets.send",
            "terminal.snippets.export",
            "terminal.snippets.import",
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
            // the toolbox
            "tools.list",
            "tools.subnet",
            "tools.secret.identify",
            "tools.oui",
            "probe.ping",
            "probe.traceroute",
            "probe.history",
            "tftp.start",
            "tools.qos.marking",
            "tools.qos.voice",
            // addressing
            "ipam.prefixes",
            "ipam.next_prefix",
            "ipam.next_address",
            "ipam.sync",
            "ipam.reconcile",
            "ipam.conflicts",
            "ipam.vlans",
            "ipam.summary",
            // the topology
            "topology.get",
            "topology.path",
            "topology.neighbors",
            "topology.export",
            "topology.rates",
            "locate.host",
            // the network designer
            "design.examples",
            "design.validate",
            "design.generate",
            "design.create_lab",
            "design.export",
            // insights
            "insights.scan",
            "insights.list",
            "insights.checks",
            "insights.coverage",
            "insights.dismiss",
            "insights.fix",
            // operations
            "overview.get",
            "incidents.create",
            "incidents.note",
            "incidents.resolve",
            "handover.generate",
            "reports.generate",
            "notes.save",
            "search.query",
            "audit.search",
            "evidence.capture_terminal",
            "evidence.capture_log",
            "evidence.capture_topology",
            "evidence.capture_chart",
            "evidence.add_text",
            "evidence.list",
            "evidence.get",
            "evidence.rename",
            "evidence.attach",
            "evidence.detach",
            "evidence.delete",
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

    /// The edition line, checked rather than trusted.
    ///
    /// A free build must carry no trace of somebody else's subscribers: not a
    /// method that answers, not a method that refuses politely. The screens
    /// disappear on their own because every one of them gates on
    /// `app.methods`, so this assertion is also what makes the interface
    /// right.
    #[cfg(not(feature = "isp"))]
    #[tokio::test]
    async fn the_free_edition_knows_nothing_about_billing_anybody() {
        let app = start(AppConfig::in_memory()).unwrap();
        let billing: Vec<&str> = app
            .methods()
            .into_iter()
            .filter(|method| method.starts_with("isp."))
            .collect();
        assert!(
            billing.is_empty(),
            "the free edition registered {billing:?}"
        );
        // And the settings screen must not offer a section for it either.
        let settings = app
            .rpc("settings.all", serde_json::Value::Null)
            .await
            .unwrap();
        assert!(
            settings.get("isp").is_none(),
            "the free edition declared an `isp` settings section"
        );
    }

    /// And the paid build must actually have it — the same assertion from the
    /// other side, so that `--features isp` cannot quietly become a no-op.
    #[cfg(feature = "isp")]
    #[tokio::test]
    async fn the_isp_edition_has_the_billing_methods_and_the_rest_of_the_build() {
        let app = start(AppConfig::in_memory()).unwrap();
        let methods = app.methods();
        for expected in [
            "isp.customers",
            "isp.invoices",
            "isp.payments",
            "isp.arrears",
            "isp.summary",
        ] {
            assert!(methods.contains(&expected), "`{expected}` is missing");
        }
        // The NOC features are not traded away for them.
        for expected in ["devices.list", "monitor.status", "changes.list"] {
            assert!(methods.contains(&expected), "`{expected}` is missing");
        }
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

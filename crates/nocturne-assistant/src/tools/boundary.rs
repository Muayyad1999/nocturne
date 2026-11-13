//! The security boundary of the product's AI.
//!
//! The model can only do what its tools do, and its tools can only call the
//! RPC methods named here. [`Method`] cannot be built outside this module —
//! there is no constructor that takes a string, so no name the model
//! influences can become a method call — and every tool declares the subset it
//! may use, which the gate enforces call by call.
//!
//! What this list must never contain: `changes.approve`, `changes.apply`,
//! `changes.update`, `credentials.*`, `vault.*`, `hostkeys.*`, `settings.set`,
//! `profiles.save`/`.delete`, `devices.create`/`.update`/`.delete`,
//! `terminal.input*` (typing into a human's terminal would bypass the command
//! policy), `sftp.*`, `tftp.*`, `labs.expose`, `schedules.*`, `ansible.run`,
//! anything that writes a file, and anything added later unless this list is
//! edited deliberately.
//!
//! `changes.assistant_apply` is on the list; whether it actually applies
//! anything is decided inside the change engine's autonomy policy, never here
//! and never by the model.

/// An RPC method a tool is allowed to call.
///
/// The field is private on purpose: the only values that exist are the
/// constants below.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Method(&'static str);

impl Method {
    pub const fn name(self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for Method {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

macro_rules! methods {
    ($($konst:ident => $name:literal,)*) => {
        $(pub const $konst: Method = Method($name);)*

        /// Every method the tool layer may reach. A test asserts that this is
        /// exactly the union of what the registered tools declare.
        pub const ALLOWED: &[Method] = &[$($konst,)*];
    };
}

methods! {
    // --- inventory, device reads ------------------------------------------
    DEVICES_LIST => "devices.list",
    DEVICES_GET => "devices.get",
    DEVICES_EXEC => "devices.exec",
    COLLECT_RUN => "collect.run",
    COLLECT_GET => "collect.get",
    JOBS_GET => "jobs.get",
    // --- stored configuration ---------------------------------------------
    CONFIG_LATEST => "config.latest",
    CONFIG_SEARCH => "config.search",
    CONFIG_DIFF => "config.diff",
    CONFIG_BACKUP => "config.backup",
    COMPLIANCE_RESULTS => "compliance.results",
    // --- monitoring --------------------------------------------------------
    MONITOR_DEVICE => "monitor.device",
    MONITOR_INTERFACES => "monitor.interfaces",
    METRICS_QUERY => "metrics.query",
    ALERTS_LIST => "alerts.list",
    LOGS_QUERY => "logs.query",
    INSIGHTS_LIST => "insights.list",
    // --- topology and addressing -------------------------------------------
    TOPOLOGY_NEIGHBORS => "topology.neighbors",
    TOPOLOGY_PATH => "topology.path",
    LOCATE_HOST => "locate.host",
    IPAM_SEARCH => "ipam.search",
    IPAM_NEXT_ADDRESS => "ipam.next_address",
    TOOLS_SUBNET => "tools.subnet",
    TOOLS_VLSM => "tools.vlsm",
    // --- probes ------------------------------------------------------------
    PROBE_PING => "probe.ping",
    PROBE_TRACEROUTE => "probe.traceroute",
    PROBE_PORTS => "probe.ports",
    PROBE_DNS => "probe.dns",
    PROBE_TLS => "probe.tls",
    // --- templates and device types ----------------------------------------
    TEMPLATES_LIST => "templates.list",
    TEMPLATES_RENDER => "templates.render",
    PROFILES_VALIDATE => "profiles.validate",
    // --- changes: draft and plan; applying is the engine's decision ---------
    CHANGES_CREATE => "changes.create",
    CHANGES_PLAN => "changes.plan",
    CHANGES_GET => "changes.get",
    CHANGES_LIST => "changes.list",
    CHANGES_PREVIEW => "changes.preview",
    CHANGES_ASSISTANT_APPLY => "changes.assistant_apply",
    // --- design and labs ----------------------------------------------------
    DESIGN_GENERATE => "design.generate",
    DESIGN_CREATE_LAB => "design.create_lab",
    LABS_LIST => "labs.list",
    LABS_CONNECTIVITY => "labs.connectivity",
    LABS_PROBE => "labs.probe",
    // --- the team's own words ------------------------------------------------
    NOTES_LIST => "notes.list",
    TERMINAL_SNAPSHOT => "terminal.snapshot",
}

/// Is this method name on the allow-list? For tests and for the `ai.tools`
/// method; the gate itself compares [`Method`] values, not strings.
pub fn is_allowed(name: &str) -> bool {
    ALLOWED.iter().any(|method| method.name() == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_forbidden_methods_are_absent() {
        for forbidden in [
            "changes.approve",
            "changes.apply",
            "changes.update",
            "changes.unapprove",
            "changes.rollback",
            "changes.cancel",
            "changes.delete",
            "credentials.list",
            "credentials.create",
            "credentials.update",
            "credentials.delete",
            "vault.unlock",
            "vault.lock",
            "vault.set_password",
            "vault.state",
            "hostkeys.list",
            "hostkeys.trust",
            "hostkeys.forget",
            "settings.set",
            "settings.get",
            "settings.all",
            "profiles.save",
            "profiles.delete",
            "devices.create",
            "devices.update",
            "devices.delete",
            "devices.bulk_update",
            "devices.import",
            "devices.export",
            "terminal.input",
            "terminal.input_many",
            "terminal.open",
            "terminal.close",
            "terminal.send_break",
            "sftp.open",
            "sftp.download",
            "tftp.status",
            "tftp.stop",
            "labs.expose",
            "labs.create",
            "labs.delete",
            "labs.start",
            "labs.stop",
            "labs.node.config",
            "schedules.list",
            "schedules.run_now",
            "ansible.run",
            "playbooks.run",
            "runner.run",
            "config.export",
            "config.restore",
            "config.delete",
            "compliance.remediate",
            "insights.fix",
            "templates.save",
            "notes.save",
            "keys.generate",
            "discovery.scan",
            "ai.send",
            "ai.providers.save",
            "app.rpc",
        ] {
            assert!(
                !is_allowed(forbidden),
                "`{forbidden}` must never be reachable from a tool"
            );
        }
    }

    #[test]
    fn the_allow_list_is_what_the_specification_fixes() {
        let mut names: Vec<&str> = ALLOWED.iter().map(|method| method.name()).collect();
        names.sort_unstable();
        let mut unique = names.clone();
        unique.dedup();
        assert_eq!(names, unique, "no method is listed twice");
        assert_eq!(
            names,
            vec![
                "alerts.list",
                "changes.assistant_apply",
                "changes.create",
                "changes.get",
                "changes.list",
                "changes.plan",
                "changes.preview",
                "collect.get",
                "collect.run",
                "compliance.results",
                "config.backup",
                "config.diff",
                "config.latest",
                "config.search",
                "design.create_lab",
                "design.generate",
                "devices.exec",
                "devices.get",
                "devices.list",
                "insights.list",
                "ipam.next_address",
                "ipam.search",
                "jobs.get",
                "labs.connectivity",
                "labs.list",
                "labs.probe",
                "locate.host",
                "logs.query",
                "metrics.query",
                "monitor.device",
                "monitor.interfaces",
                "notes.list",
                "probe.dns",
                "probe.ping",
                "probe.ports",
                "probe.tls",
                "probe.traceroute",
                "profiles.validate",
                "templates.list",
                "templates.render",
                "terminal.snapshot",
                "tools.subnet",
                "tools.vlsm",
                "topology.neighbors",
                "topology.path",
            ]
        );
    }

    #[test]
    fn nothing_that_writes_to_a_device_or_a_secret_is_on_the_list() {
        for method in ALLOWED {
            let name = method.name();
            assert!(
                !name.starts_with("credentials.")
                    && !name.starts_with("vault.")
                    && !name.starts_with("hostkeys.")
                    && !name.starts_with("sftp.")
                    && !name.starts_with("tftp.")
                    && !name.starts_with("terminal.input")
                    && !name.starts_with("schedules.")
                    && !name.starts_with("ai."),
                "`{name}` is on the allow-list"
            );
        }
    }
}

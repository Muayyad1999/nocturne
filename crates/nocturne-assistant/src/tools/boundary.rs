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
//!
//! Three kinds of method are on the list under a condition, and the gate
//! (`ToolCtx::rpc`) holds each to it call by call:
//!
//! * [`ASK_FIRST`] — the three that write a record which is not a device's
//!   configuration (acknowledge an alert, reserve an address, start a scan).
//!   Each is reachable only after the person has said yes, in this very tool
//!   call, to a question that shows what will be written. No setting turns
//!   the question off.
//! * [`LEDGER`] — the ISP edition's customer ledger: other people's data.
//!   Reachable only while the `isp_access` setting is on, and what comes back
//!   is cut down to an allow-list of fields by the tools that read it.
//! * [`ALREADY_REDACTED`] — records another feature redacted before it kept
//!   them. Their placeholders are numbered in that feature's own table, not
//!   in this chat's, so the gate takes the numbers away before a model can
//!   hand one back and have a different secret restored in its place.
//!
//! Everything else that writes stays off the list: nothing here adds to an
//! incident, captures or deletes evidence, runs automation, switches a
//! detection off, types into a terminal, or touches the ISP ledger.

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
    // --- logs and detections: what was shouting, and what else happened ------
    LOGS_BURSTS => "logs.bursts",
    LOGS_AROUND => "logs.around",
    DETECTIONS_RECENT => "detections.recent",
    DETECTIONS_LIST => "detections.list",
    CHECKS_LIST => "checks.list",
    // --- operations records: read, never added to ----------------------------
    INCIDENTS_LIST => "incidents.list",
    INCIDENTS_GET => "incidents.get",
    HANDOVER_GENERATE => "handover.generate",
    EVIDENCE_LIST => "evidence.list",
    EVIDENCE_GET => "evidence.get",
    // --- discovery and automation: what was found, how a run went ------------
    DISCOVERY_SCANS => "discovery.scans",
    DISCOVERY_SCAN_GET => "discovery.scan_get",
    TASKS_LIST => "tasks.list",
    RUNNER_RUNS => "runner.runs",
    // --- records the assistant may write, each only after a yes --------------
    ALERTS_GET => "alerts.get",
    ALERTS_ACK => "alerts.ack",
    IPAM_ADDRESS_RESERVE => "ipam.address.reserve",
    DISCOVERY_SCAN => "discovery.scan",
    // --- the ISP ledger: reads only, and only while `isp_access` is on -------
    ISP_SUMMARY => "isp.summary",
    ISP_CUSTOMERS => "isp.customers",
    ISP_SUBSCRIPTIONS => "isp.subscriptions",
    ISP_ARREARS => "isp.arrears",
    ISP_INVOICES => "isp.invoices",
}

/// Methods that write a record. The gate lets a tool reach one only after
/// the person answered yes to that tool call's own question; one yes covers
/// one write.
pub const ASK_FIRST: &[Method] = &[ALERTS_ACK, IPAM_ADDRESS_RESERVE, DISCOVERY_SCAN];

/// The ISP ledger. The gate refuses these while `isp_access` is off, and a
/// tool that needs one is not offered at all.
pub const LEDGER: &[Method] = &[
    ISP_SUMMARY,
    ISP_CUSTOMERS,
    ISP_SUBSCRIPTIONS,
    ISP_ARREARS,
    ISP_INVOICES,
];

/// Methods whose answers were redacted by the feature that stores them,
/// with placeholders from that feature's own table.
pub const ALREADY_REDACTED: &[Method] = &[
    INCIDENTS_LIST,
    INCIDENTS_GET,
    HANDOVER_GENERATE,
    EVIDENCE_LIST,
    EVIDENCE_GET,
];

pub fn asks_first(method: Method) -> bool {
    ASK_FIRST.contains(&method)
}

pub fn is_ledger(method: Method) -> bool {
    LEDGER.contains(&method)
}

pub fn already_redacted(method: Method) -> bool {
    ALREADY_REDACTED.contains(&method)
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
            "ai.send",
            "ai.providers.save",
            "app.rpc",
            // Evidence is read, never captured, renamed, attached or deleted.
            "evidence.capture_terminal",
            "evidence.capture_log",
            "evidence.capture_topology",
            "evidence.capture_chart",
            "evidence.add_text",
            "evidence.rename",
            "evidence.attach",
            "evidence.detach",
            "evidence.delete",
            // Snippets are suggested; sending one types into a terminal.
            "terminal.snippets.send",
            "terminal.snippets.import",
            "terminal.snippets.save",
            "terminal.snippets.delete",
            // Switching a detection off is how an intruder would start.
            "detections.set_enabled",
            // Log lines are evidence: nothing writes one, clears them or
            // sends them elsewhere.
            "logs.ingest",
            "logs.clear",
            "logs.pull",
            "logs.restart",
            "logs.rules.save",
            "logs.rules.delete",
            "logs.forward.list",
            "logs.forward.save",
            "logs.forward.delete",
            "logs.forward.test",
            // Incidents and handovers are a person's record of what happened.
            "incidents.create",
            "incidents.update",
            "incidents.delete",
            "incidents.note",
            "incidents.link",
            "incidents.sync",
            "incidents.acknowledge",
            "incidents.assign",
            "incidents.resolve",
            "incidents.reopen",
            "incidents.export",
            "handover.save",
            "handover.delete",
            "handover.export",
            // Acknowledging asks first; everything else about an alert or a
            // check stays a person's.
            "alerts.raise",
            "alerts.resolve",
            "alerts.snooze",
            "alerts.rules.save",
            "alerts.rules.delete",
            "checks.save",
            "checks.delete",
            "checks.run_now",
            // A scan of a range asks first; adding what it found, crawling
            // through managed devices and forgetting a scan do not exist here.
            "discovery.add",
            "discovery.crawl",
            "discovery.ignore",
            "discovery.unignore",
            "discovery.scan_delete",
            // Running automation stays a person's click.
            "tasks.save",
            "tasks.delete",
            "tasks.run",
            "runner.run",
            "runner.delete",
            "runner.export",
            "triggers.save",
            "triggers.delete",
            "triggers.fire_now",
            // Reserving asks first; the rest of the address plan is not
            // written from here.
            "ipam.address.save",
            "ipam.address.delete",
            "ipam.prefix.save",
            "ipam.prefix.delete",
            "ipam.prefix.split",
            "ipam.vlan.save",
            "ipam.vlan.delete",
            "ipam.sync",
            "ipam.learn",
            // The ISP ledger: every method that writes, and the one that
            // returns a customer whole.
            "isp.customer.get",
            "isp.customer.save",
            "isp.customer.state",
            "isp.customer.delete",
            "isp.tariff.save",
            "isp.tariff.delete",
            "isp.subscription.create",
            "isp.subscription.amend",
            "isp.subscription.end",
            "isp.link.save",
            "isp.link.delete",
            "isp.invoice.draft",
            "isp.invoice.run",
            "isp.invoice.line.add",
            "isp.invoice.line.remove",
            "isp.invoice.issue",
            "isp.invoice.void",
            "isp.invoice.reissue",
            "isp.payment.record",
            "isp.payment.void",
            "isp.credit.issue",
            "isp.credit.allocate",
            "isp.credit.void",
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
                "alerts.ack",
                "alerts.get",
                "alerts.list",
                "changes.assistant_apply",
                "changes.create",
                "changes.get",
                "changes.list",
                "changes.plan",
                "changes.preview",
                "checks.list",
                "collect.get",
                "collect.run",
                "compliance.results",
                "config.backup",
                "config.diff",
                "config.latest",
                "config.search",
                "design.create_lab",
                "design.generate",
                "detections.list",
                "detections.recent",
                "devices.exec",
                "devices.get",
                "devices.list",
                "discovery.scan",
                "discovery.scan_get",
                "discovery.scans",
                "evidence.get",
                "evidence.list",
                "handover.generate",
                "incidents.get",
                "incidents.list",
                "insights.list",
                "ipam.address.reserve",
                "ipam.next_address",
                "ipam.search",
                "isp.arrears",
                "isp.customers",
                "isp.invoices",
                "isp.subscriptions",
                "isp.summary",
                "jobs.get",
                "labs.connectivity",
                "labs.list",
                "labs.probe",
                "locate.host",
                "logs.around",
                "logs.bursts",
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
                "runner.runs",
                "tasks.list",
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

    #[test]
    fn the_methods_that_write_a_record_are_exactly_the_three_that_ask_first() {
        let mut names: Vec<&str> = ASK_FIRST.iter().map(|method| method.name()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec!["alerts.ack", "discovery.scan", "ipam.address.reserve"]
        );
        for method in ASK_FIRST {
            assert!(is_allowed(method.name()) && asks_first(*method));
        }
        assert!(!asks_first(ALERTS_GET) && !asks_first(CHANGES_ASSISTANT_APPLY));
    }

    #[test]
    fn every_ledger_method_on_the_list_is_a_read_behind_the_setting() {
        let mut on_the_list: Vec<&str> = ALLOWED
            .iter()
            .map(|method| method.name())
            .filter(|name| name.starts_with("isp."))
            .collect();
        on_the_list.sort_unstable();
        let mut ledger: Vec<&str> = LEDGER.iter().map(|method| method.name()).collect();
        ledger.sort_unstable();
        assert_eq!(
            on_the_list, ledger,
            "an `isp.*` method is on the allow-list without being behind `isp_access`"
        );
        assert_eq!(
            ledger,
            vec![
                "isp.arrears",
                "isp.customers",
                "isp.invoices",
                "isp.subscriptions",
                "isp.summary",
            ]
        );
    }
}

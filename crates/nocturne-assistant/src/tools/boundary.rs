//! The security boundary of the product's AI.
//!
//! The model can only do what its tools do, and its tools can only call the
//! RPC methods named here. [`Method`] cannot be built outside this module —
//! there is no constructor that takes a string, so no name the model
//! influences can become a method call — and every tool declares the subset it
//! may use, which the gate enforces call by call.
//!
//! Every other method the application registers is on [`NEVER`], with the
//! reason it may not be reached. The two lists together are every method the
//! assembled application has: `crates/nocturne-suite/tests/assistant_tools.rs`
//! holds them to that, so a method a feature adds later fails that test until
//! somebody decides, here, which side of the line it is on.
//!
//! `changes.assistant_apply` is on the allow-list; whether it actually applies
//! anything is decided inside the change engine's autonomy policy, never here
//! and never by the model.
//!
//! Two kinds of method are on the list under a condition, and the gate
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
//!
//! And one rule holds for every answer: the placeholders in it are another
//! table's. Configurations, changes, terminal screens, log lines, evidence and
//! incident timelines are redacted by the feature that holds them, each in a
//! table of its own; the gate takes the numbers off before a model can hand
//! one back and have this chat's secret of the same number restored in its
//! place (`redact::without_foreign_placeholders`).

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
    // --- what the traffic is doing, and the team's snippets: reads -----------
    MONITOR_QOS => "monitor.qos",
    FLOWS_TOP => "flows.top",
    TERMINAL_SNIPPETS_LIST => "terminal.snippets.list",
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

/// Every method the application registers that no tool may reach, grouped by
/// the reason it may not.
///
/// This is not a filter the gate consults — the gate only ever lets through
/// what is in [`ALLOWED`] — it is the other half of a decision that has to be
/// made for every method. A method that is in neither list is one nobody has
/// decided about; the suite's test refuses the build until somebody does.
pub const NEVER: &[(&str, &[&str])] = &[
    (
        "A change is approved by a person, bound to its plan's hash; whether one is applied is \
         the change engine's decision (`changes.assistant_apply`); an undo is a change a person \
         starts; and a change a person made is theirs to edit, withdraw or delete.",
        &[
            "changes.approve",
            "changes.unapprove",
            "changes.apply",
            "changes.update",
            "changes.rollback",
            "changes.cancel",
            "changes.delete",
        ],
    ),
    (
        "Configures a device, or runs automation that may, outside the change pipeline: the \
         assistant changes a configuration only by drafting a change.",
        &[
            "compliance.remediate",
            "config.restore",
            "config.drift.fix",
            "insights.fix",
            "templates.apply",
            "ansible.run",
            "playbooks.run",
            "runner.run",
            "tasks.run",
            "schedules.run_now",
            "triggers.fire_now",
            "labs.node.config_set",
        ],
    ),
    (
        "A person's terminal and file transfer: typing into, opening, steering, attaching to (its \
         scrollback is raw and unredacted) or recording a session, and moving files to or from a \
         device. Each would go around the device type's command policy.",
        &[
            "terminal.open",
            "terminal.close",
            "terminal.attach",
            "terminal.input",
            "terminal.input_many",
            "terminal.resize",
            "terminal.send_break",
            "terminal.logging",
            "terminal.snippets.send",
            "sftp.open",
            "sftp.close",
            "sftp.list",
            "sftp.stat",
            "sftp.mkdir",
            "sftp.remove",
            "sftp.rename",
            "sftp.read_text",
            "sftp.write_text",
            "sftp.download",
            "sftp.upload",
            "sftp.sessions",
            "tftp.start",
            "tftp.status",
            "tftp.stop",
        ],
    ),
    (
        "Logins, keys and secrets, and the trust that guards them: never in front of a model, not \
         even as a list.",
        &[
            "credentials.list",
            "credentials.create",
            "credentials.update",
            "credentials.delete",
            "vault.state",
            "vault.unlock",
            "vault.lock",
            "vault.set_password",
            "hostkeys.list",
            "hostkeys.trust",
            "hostkeys.forget",
            "keys.generate",
            "tools.keys.generate",
            "tools.password",
            "tools.secret.decode",
            "tools.secret.encode",
            "tools.secret.identify",
        ],
    ),
    (
        "What the assistant and Nocturne may do and what they watch: settings (the assistant's \
         own limits among them), device types (which commands are read-only), alert, log and \
         compliance rules, maintenance windows that mute alerts, detections, schedules, triggers \
         and who is notified. Never written from here, and not read either: a notification \
         channel's address can carry a token.",
        &[
            "settings.all",
            "settings.get",
            "settings.set",
            "profiles.save",
            "profiles.delete",
            "alerts.rules.save",
            "alerts.rules.delete",
            "logs.rules.save",
            "logs.rules.delete",
            "compliance.policies.save",
            "compliance.policies.delete",
            "compliance.policies.duplicate",
            "maintenance.save",
            "maintenance.delete",
            "detections.set_enabled",
            "schedules.save",
            "schedules.delete",
            "triggers.save",
            "triggers.delete",
            "notify.channels.list",
            "notify.channels.save",
            "notify.channels.delete",
            "notify.channels.test",
        ],
    ),
    (
        "The assistant itself: its chats, its providers and its approvals. A tool that could \
         answer `ai.approve` would say yes to its own questions.",
        &[
            "ai.approve",
            "ai.cancel",
            "ai.send",
            "ai.status",
            "ai.tools",
            "ai.usage",
            "ai.presets",
            "ai.chats.create",
            "ai.chats.delete",
            "ai.chats.get",
            "ai.chats.list",
            "ai.chats.rename",
            "ai.providers.list",
            "ai.providers.save",
            "ai.providers.delete",
            "ai.providers.test",
            "ai.providers.models",
            "ai.providers.presets",
            "ai.providers.detect_local",
        ],
    ),
    (
        "Hides a problem from a person, or undoes their decision about one: clearing, resolving \
         or snoozing an alert, dismissing or reopening an insight. Raising an alert is for rules \
         and detections, not for a model.",
        &[
            "alerts.clear",
            "alerts.resolve",
            "alerts.snooze",
            "alerts.raise",
            "insights.dismiss",
            "insights.reopen",
        ],
    ),
    (
        "Writes a person's record of what happened, or the evidence for it: incidents, handovers, \
         notes, evidence, log lines and service checks.",
        &[
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
            "handover.save",
            "handover.delete",
            "notes.save",
            "notes.delete",
            "evidence.capture_terminal",
            "evidence.capture_log",
            "evidence.capture_topology",
            "evidence.capture_chart",
            "evidence.add_text",
            "evidence.rename",
            "evidence.attach",
            "evidence.detach",
            "evidence.delete",
            "logs.ingest",
            "logs.clear",
            "logs.pull",
            "checks.save",
            "checks.delete",
        ],
    ),
    (
        "Sends what Nocturne collects somewhere else, or restarts what receives it: log \
         forwarding and the receivers.",
        &[
            "logs.forward.list",
            "logs.forward.save",
            "logs.forward.delete",
            "logs.forward.test",
            "logs.restart",
        ],
    ),
    (
        "Changes the inventory, the address plan, the topology, a lab or another store a person \
         keeps, or stops a person's job. A lab is changed like any network: through a change.",
        &[
            "devices.create",
            "devices.update",
            "devices.delete",
            "devices.bulk_update",
            "devices.import",
            "discovery.add",
            "discovery.crawl",
            "discovery.ignore",
            "discovery.unignore",
            "discovery.scan_delete",
            "ipam.address.save",
            "ipam.address.delete",
            "ipam.prefix.save",
            "ipam.prefix.delete",
            "ipam.prefix.split",
            "ipam.vlan.save",
            "ipam.vlan.delete",
            "ipam.import",
            "ipam.learn",
            "ipam.sync",
            "topology.layout.save",
            "topology.manual.save",
            "topology.manual.delete",
            "topology.view_save",
            "topology.view_delete",
            "templates.save",
            "templates.delete",
            "templates.duplicate",
            "playbooks.save",
            "playbooks.delete",
            "tasks.save",
            "tasks.delete",
            "runner.delete",
            "reports.save",
            "reports.delete",
            "probe.history.delete",
            "terminal.logs.delete",
            "terminal.snippets.save",
            "terminal.snippets.delete",
            "terminal.snippets.import",
            "config.delete",
            "config.pin",
            "config.unpin",
            "config.intended.set",
            "config.intended.clear",
            "jobs.cancel",
            "jobs.clear",
            "labs.create",
            "labs.update",
            "labs.delete",
            "labs.duplicate",
            "labs.import",
            "labs.start",
            "labs.stop",
            "labs.reset",
            "labs.speed",
            "labs.capture",
            "labs.twin",
            "labs.link.add",
            "labs.link.remove",
            "labs.node.add",
            "labs.node.update",
            "labs.node.move",
            "labs.node.remove",
            "labs.flow.add",
            "labs.flow.remove",
            "labs.fault.inject",
            "labs.fault.clear",
            "labs.unexpose",
        ],
    ),
    ("Puts a lab device on the real network.", &["labs.expose"]),
    (
        "Writes files, or hands a whole store out as a document: what a person exports is theirs \
         to send on.",
        &[
            "config.export",
            "ansible.export_playbook",
            "devices.export",
            "ipam.export",
            "labs.export",
            "runner.export",
            "handover.export",
            "incidents.export",
            "terminal.snippets.export",
            "design.export",
        ],
    ),
    (
        "Starts work on devices or on the network that no tool needs, or that a tool does in its \
         own way: a poll, a login test, a check, a compliance run, a scan or a report run now; a \
         probe from a device (the tools run the device's own command, under its command \
         policy); a sweep of addresses (`start_discovery` asks first); any URL; a lookup at an \
         outside service; a wake-on-LAN packet; a syslog listener; a lab trace; a live log tail \
         held open on the server.",
        &[
            "monitor.poll_now",
            "checks.run_now",
            "devices.test",
            "compliance.run",
            "insights.scan",
            "reports.generate",
            "probe.device.ping",
            "probe.device.traceroute",
            "probe.sweep",
            "probe.http",
            "probe.ntp",
            "probe.whois",
            "probe.wol",
            "probe.syslog.start",
            "probe.syslog.stop",
            "probe.syslog.status",
            "probe.syslog.messages",
            "labs.trace",
            "logs.follow",
            "logs.unfollow",
        ],
    ),
    (
        "The ISP ledger beyond the five reads behind `isp_access`: every method that writes, \
         `isp.customer.get` (a customer whole, contact details included), and reads that no tool \
         shapes from an allow-list of fields.",
        &[
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
            "isp.invoice.get",
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
            "isp.capacity",
            "isp.credits",
            "isp.currencies",
            "isp.links",
            "isp.margin",
            "isp.payments",
            "isp.revenue",
            "isp.tariffs",
            "isp.usage",
        ],
    ),
    (
        "The audit trail: the record of what people, and the assistant, did. It is evidence, not \
         something to put in front of a model that may be steered by what it reads.",
        &["audit.actions", "audit.list", "audit.search"],
    ),
    (
        "Internal plumbing between features, not an answer for anybody: ifIndex numbers, the \
         method table.",
        &["monitor.ifindex", "app.methods"],
    ),
    (
        "A note, a stored handover or a recorded terminal session whole: the team's own text and \
         what a device printed to one person. The tools show notes as titles and matching \
         passages; a person opens the rest.",
        &[
            "notes.get",
            "handover.get",
            "handover.list",
            "terminal.logs.list",
            "terminal.logs.read",
        ],
    ),
    (
        "Fills a snippet in: that is the person's step before anything is typed. The assistant \
         suggests a snippet (`list_snippets`); it does not prepare text for a terminal.",
        &["terminal.snippets.render"],
    ),
    (
        "A read no tool is built on. A method joins the allow-list together with a tool that \
         shapes its answer for a model, never on its own.",
        &[
            "alerts.rules.list",
            "alerts.rules.test",
            "alerts.summary",
            "ansible.status",
            "app.info",
            "checks.get",
            "checks.history",
            "checks.kinds",
            "collect.status",
            "compliance.policies.get",
            "compliance.policies.list",
            "compliance.policies.validate",
            "compliance.runs",
            "compliance.test",
            "config.compare",
            "config.devices",
            "config.drift",
            "config.get",
            "config.intended.get",
            "config.missing",
            "config.unsaved",
            "config.versions",
            "design.examples",
            "design.validate",
            "devices.facets",
            "devices.import_preview",
            "discovery.ignored",
            "discovery.preview",
            "discovery.suggest",
            "flows.exporters",
            "incidents.suggest_links",
            "insights.checks",
            "insights.coverage",
            "insights.get",
            "ipam.address.get",
            "ipam.conflicts",
            "ipam.next_prefix",
            "ipam.prefix.get",
            "ipam.prefixes",
            "ipam.reconcile",
            "ipam.summary",
            "ipam.vlans",
            "jobs.list",
            "labs.catalog",
            "labs.examples",
            "labs.exposed",
            "labs.fault.list",
            "labs.flow.list",
            "labs.get",
            "labs.links",
            "labs.node.config",
            "labs.node.metrics",
            "labs.node.tables",
            "labs.packets",
            "logs.device_setup",
            "logs.histogram",
            "logs.rules.list",
            "logs.rules.test",
            "logs.sources",
            "logs.status",
            "logs.top",
            "maintenance.list",
            "metrics.query_many",
            "metrics.series",
            "metrics.top",
            "monitor.status",
            "overview.get",
            "playbooks.get",
            "playbooks.list",
            "playbooks.run_get",
            "playbooks.runs",
            "playbooks.validate",
            "probe.history",
            "probe.history.get",
            "probe.local",
            "profiles.get",
            "profiles.list",
            "reports.get",
            "reports.kinds",
            "reports.list",
            "runner.compare",
            "runner.get",
            "runner.search",
            "schedules.list",
            "schedules.preview",
            "search.query",
            "tasks.devices",
            "tasks.get",
            "templates.get",
            "templates.validate",
            "terminal.classify",
            "terminal.list",
            "terminal.serial_ports",
            "topology.autolayout",
            "topology.changes",
            "topology.export",
            "topology.get",
            "topology.manual.list",
            "topology.rates",
            "topology.views",
            "triggers.get",
            "triggers.list",
            "tools.base64",
            "tools.contains",
            "tools.convert",
            "tools.diff",
            "tools.hash",
            "tools.interface",
            "tools.ip.convert",
            "tools.ip.range",
            "tools.list",
            "tools.mac",
            "tools.mtu",
            "tools.oid",
            "tools.oui",
            "tools.overlaps",
            "tools.qos.marking",
            "tools.qos.voice",
            "tools.reference",
            "tools.reference.tables",
            "tools.regex",
            "tools.subnet.split",
            "tools.supernet",
            "tools.transfer_time",
            "tools.wildcard",
            "tools.wildcard.acl",
        ],
    ),
];

pub fn asks_first(method: Method) -> bool {
    ASK_FIRST.contains(&method)
}

pub fn is_ledger(method: Method) -> bool {
    LEDGER.contains(&method)
}

/// Is this method name on the allow-list? For tests and for the `ai.tools`
/// method; the gate itself compares [`Method`] values, not strings.
pub fn is_allowed(name: &str) -> bool {
    ALLOWED.iter().any(|method| method.name() == name)
}

/// Why `name` may never be reached from a tool, when it is on [`NEVER`].
pub fn never_reachable(name: &str) -> Option<&'static str> {
    NEVER
        .iter()
        .find(|(_, names)| names.contains(&name))
        .map(|(reason, _)| *reason)
}

/// Every name on [`NEVER`], in the order the list gives them.
pub fn never_names() -> impl Iterator<Item = &'static str> {
    NEVER.iter().flat_map(|(_, names)| names.iter().copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_forbidden_methods_are_absent() {
        // The ones the specification names, and a few a model might hope for.
        for forbidden in [
            "changes.approve",
            "changes.apply",
            "changes.update",
            "changes.unapprove",
            "changes.rollback",
            "changes.cancel",
            "changes.delete",
            "credentials.list",
            "vault.unlock",
            "hostkeys.trust",
            "settings.set",
            "settings.get",
            "profiles.save",
            "profiles.delete",
            "devices.create",
            "devices.update",
            "devices.delete",
            "terminal.input",
            "terminal.input_many",
            "terminal.open",
            "sftp.open",
            "tftp.start",
            "labs.expose",
            "schedules.run_now",
            "ansible.run",
            "playbooks.run",
            "runner.run",
            "config.export",
            "config.restore",
            "keys.generate",
            "ai.send",
            "ai.approve",
            "app.rpc",
            "alerts.clear",
            "monitor.ifindex",
            "logs.ingest",
            "logs.forward.list",
            "detections.set_enabled",
            "terminal.snippets.send",
            "terminal.snippets.render",
            "terminal.snippets.import",
            "evidence.add_text",
            "isp.customer.get",
        ] {
            assert!(
                !is_allowed(forbidden),
                "`{forbidden}` must never be reachable from a tool"
            );
        }
        // Every name on the never-reachable list is off the allow-list, has
        // a reason, and is there once.
        let mut names: Vec<&str> = never_names().collect();
        for name in &names {
            assert!(!is_allowed(name), "`{name}` is on both lists");
            assert!(
                never_reachable(name).is_some_and(|reason| reason.len() > 20),
                "`{name}` has no reason"
            );
        }
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "a method is on the never list twice");
        // The methods the specification names are on it by name, not only
        // absent from the other list.
        for named in [
            "alerts.clear",
            "monitor.ifindex",
            "logs.ingest",
            "logs.forward.list",
            "logs.forward.save",
            "logs.forward.delete",
            "logs.forward.test",
            "flows.exporters",
            "detections.set_enabled",
            "terminal.snippets.render",
            "terminal.snippets.send",
            "terminal.snippets.export",
            "terminal.snippets.import",
            "evidence.capture_terminal",
            "evidence.delete",
            "tools.qos.marking",
            "tools.qos.voice",
            "topology.rates",
            "changes.rollback",
            "changes.approve",
        ] {
            assert!(never_reachable(named).is_some(), "`{named}` is not named");
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
                "flows.top",
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
                "monitor.qos",
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
                "terminal.snippets.list",
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

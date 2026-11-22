//! Every check Nocturne ships, what it needs, and what it cannot see.
//!
//! `description` is written for the person deciding whether to trust a
//! finding, and `not_evaluated` says exactly when the check declines to
//! answer. Both are part of the UI, because a check whose blind spots are not
//! written down is a check nobody can act on.

use serde::Serialize;

use crate::checks;
use crate::model::{Category, Severity};
use crate::report::CheckReport;
use crate::settings::Settings;
use crate::world::World;

/// What a check does, over data it declares.
pub struct Check {
    pub id: &'static str,
    pub title: &'static str,
    pub category: Category,
    pub severity: Severity,
    /// Collected kinds and other sources it reads, for the catalogue screen.
    pub needs: &'static [&'static str],
    pub description: &'static str,
    /// When the check reports "not evaluated" instead of "nothing found".
    pub not_evaluated: &'static str,
    pub default_enabled: bool,
    pub run: fn(&World, &Settings, &mut CheckReport),
}

/// A check as `insights.checks` returns it.
#[derive(Clone, Debug, Serialize)]
pub struct CheckInfo {
    pub id: &'static str,
    pub title: &'static str,
    pub category: Category,
    pub default_severity: Severity,
    pub needs: &'static [&'static str],
    pub enabled: bool,
    pub description: &'static str,
    pub not_evaluated: &'static str,
}

pub fn get(id: &str) -> Option<&'static Check> {
    CHECKS.iter().find(|check| check.id == id)
}

pub fn info(settings: &Settings) -> Vec<CheckInfo> {
    CHECKS
        .iter()
        .map(|check| CheckInfo {
            id: check.id,
            title: check.title,
            category: check.category,
            default_severity: check.severity,
            needs: check.needs,
            enabled: settings.enabled(check.id),
            description: check.description,
            not_evaluated: check.not_evaluated,
        })
        .collect()
}

const INTERFACES: &[&str] = &["interfaces"];
const LINK: &[&str] = &["interfaces", "neighbors (or the topology feature)"];

pub static CHECKS: &[Check] = &[
    // ----------------------------------------------------------- physical
    Check {
        id: "duplex_mismatch",
        title: "Duplex mismatch",
        category: Category::Physical,
        severity: Severity::High,
        needs: LINK,
        description: "The two ends of a link settled on different duplex, which gives late \
            collisions, CRC errors and a link that works until it is busy. Only resolved duplex \
            counts: a port still auto-negotiating says `auto`, which is not a verdict. Half duplex \
            at 10 Mb/s is left alone — on legacy equipment it is the normal outcome.",
        not_evaluated: "No link to another managed device, or either end does not report a \
            resolved duplex (`auto`, empty, or a spelling this check does not know).",
        default_enabled: true,
        run: checks::physical::duplex_mismatch,
    },
    Check {
        id: "half_duplex_uplink",
        title: "Half duplex on a port with a neighbour",
        category: Category::Physical,
        severity: Severity::Medium,
        needs: &["interfaces", "neighbors"],
        description: "A port that another network device is plugged into is running half duplex \
            at 100 Mb/s or more. Even where the far end agrees, this is almost always a forced \
            setting left behind on one side. Ports whose far end is also managed are left to \
            `duplex_mismatch`.",
        not_evaluated: "No neighbour was collected for the port, or its duplex is not resolved.",
        default_enabled: true,
        run: checks::physical::half_duplex_uplink,
    },
    Check {
        id: "speed_mismatch",
        title: "Link speed mismatch",
        category: Category::Physical,
        severity: Severity::High,
        needs: LINK,
        description: "The two ends of an up link report different speeds. Down ports are not \
            compared: what a down port calls its speed is the configured maximum, not the speed \
            of a link.",
        not_evaluated: "No link, either end is not up, or either end reports no speed.",
        default_enabled: true,
        run: checks::physical::speed_mismatch,
    },
    Check {
        id: "mtu_mismatch",
        title: "MTU mismatch across a link",
        category: Category::Physical,
        severity: Severity::Medium,
        needs: LINK,
        description: "The two ends of a link are configured with different MTUs, which breaks \
            large frames while small ones work. Differences of 18 bytes or less are ignored: \
            vendors disagree about whether the Ethernet header and FCS are part of the MTU, and \
            1500 against 1514 is the same link.",
        not_evaluated: "No link, either end is not up, or either end reports no MTU.",
        default_enabled: true,
        run: checks::physical::mtu_mismatch,
    },
    Check {
        id: "interface_errors",
        title: "Input errors growing",
        category: Category::Physical,
        severity: Severity::Medium,
        needs: &["interfaces", "the previous collection of interfaces"],
        description: "Input errors or CRCs grew between the last two collections, fast enough to \
            matter against the traffic the port carried. A counter that is merely non-zero is not \
            a finding: every port that has ever been unplugged has errors since boot.",
        not_evaluated: "Only one collection, no error counters, or the counters went backwards \
            (`clear counters`, a reboot, a counter wrap) — none of which can be read as a rate.",
        default_enabled: true,
        run: checks::physical::interface_errors,
    },
    Check {
        id: "interface_flapping",
        title: "Port flapping",
        category: Category::Physical,
        severity: Severity::High,
        needs: &["`if.oper` samples (monitoring), or link up/down log entries"],
        description: "The link went up and down repeatedly within an hour — a failing transceiver, \
            a damaged cable, or a device at the far end rebooting in a loop. Counted from polled \
            `if.oper` samples when monitoring runs, otherwise from link up/down log lines.",
        not_evaluated: "Neither monitoring samples nor log entries exist for the device in the \
            window; a port that was simply never polled is never called stable.",
        default_enabled: true,
        run: checks::physical::interface_flapping,
    },
    Check {
        id: "err_disabled",
        title: "Port in err-disabled state",
        category: Category::Physical,
        severity: Severity::High,
        needs: &["link error log entries", "interfaces"],
        description: "The switch disabled a port itself — BPDU guard, port security, a link flap \
            threshold or a loop — and it is still down. Read from the device's own log lines, with \
            the cause the platform printed; a port that has since come back up is not reported.",
        not_evaluated: "No log entries from the device in the window (no syslog receiver, or the \
            device does not send to it), or no interface data to confirm the port is still down.",
        default_enabled: true,
        run: checks::physical::err_disabled,
    },
    Check {
        id: "down_uplink",
        title: "Link to a known neighbour is down",
        category: Category::Physical,
        severity: Severity::High,
        needs: &[
            "interfaces",
            "neighbors",
            "the previous collection of neighbors",
        ],
        description: "A port that a network device was plugged into is administratively up but \
            the link is down. Ports that were shut are not reported — that is somebody's decision, \
            not a fault.",
        not_evaluated: "No neighbour was ever collected for the port, or the port's state is \
            unknown.",
        default_enabled: true,
        run: checks::physical::down_uplink,
    },
    Check {
        id: "high_utilisation",
        title: "Interface sustained over its utilisation threshold",
        category: Category::Capacity,
        severity: Severity::Medium,
        needs: &["`if.util_pct` samples (monitoring)"],
        description: "The port spent most of a long stretch above the utilisation threshold, with \
            the hour of day it peaks in. A single busy poll is not congestion.",
        not_evaluated: "No utilisation samples for the device, or too short a history to call \
            anything sustained.",
        default_enabled: true,
        run: checks::physical::high_utilisation,
    },
    // ---------------------------------------------------------------- l2
    Check {
        id: "native_vlan_mismatch",
        title: "Native VLAN mismatch on a trunk",
        category: Category::L2,
        severity: Severity::High,
        needs: &[
            "interfaces",
            "neighbors (or the topology feature)",
            "running configuration",
        ],
        description: "The two ends of a trunk put untagged frames in different VLANs, which \
            bridges two VLANs together. Taken from the collected trunk native VLAN, or from \
            `switchport trunk native vlan` in the configuration — where an IOS-style trunk has no \
            such line the native VLAN is 1 by definition.",
        not_evaluated: "Either end is not known to be a trunk, or its native VLAN is neither \
            collected nor derivable from a configuration whose grammar this check knows.",
        default_enabled: true,
        run: checks::l2::native_vlan_mismatch,
    },
    Check {
        id: "trunk_vlan_mismatch",
        title: "Trunk ends allow different VLANs",
        category: Category::L2,
        severity: Severity::Medium,
        needs: &[
            "running configuration",
            "neighbors (or the topology feature)",
            "vlans",
        ],
        description: "Both ends of a trunk restrict the VLANs they carry, and the two lists \
            differ, so traffic in the odd VLANs is dropped at one end. One end allowing everything \
            and the other a list is *not* reported: that is how a pruned trunk is normally built.",
        not_evaluated: "No link, a configuration grammar the check does not read, or either end \
            has no explicit allowed list.",
        default_enabled: true,
        run: checks::l2::trunk_vlan_mismatch,
    },
    Check {
        id: "vlan_not_on_trunk",
        title: "VLAN with access ports is not carried by any trunk",
        category: Category::L2,
        severity: Severity::High,
        needs: &["vlans", "interfaces", "running configuration"],
        description: "Hosts are in a VLAN on this switch that none of its trunks carry, so they \
            can reach each other and nothing else. Only switches that have at least one trunk with \
            an explicit allowed list are examined.",
        not_evaluated: "No trunk with an explicit allowed list (a trunk that allows everything \
            carries the VLAN), no VLAN list, or a configuration grammar the check does not read.",
        default_enabled: true,
        run: checks::l2::vlan_not_on_trunk,
    },
    Check {
        id: "unused_vlan",
        title: "VLAN defined but unused",
        category: Category::Hygiene,
        severity: Severity::Info,
        needs: &["vlans", "interfaces", "running configuration"],
        description: "A VLAN exists on the switch with no access port, no SVI and no trunk \
            carrying it. Tidiness, not a fault. A VLAN that any trunk allows is never reported — \
            it may be transiting for a switch further down.",
        not_evaluated: "No VLAN list, or a trunk allows every VLAN, which would make any verdict \
            a guess.",
        default_enabled: true,
        run: checks::l2::unused_vlan,
    },
    Check {
        id: "vlan_name_inconsistent",
        title: "The same VLAN id is named differently",
        category: Category::Hygiene,
        severity: Severity::Info,
        needs: &["vlans"],
        description: "One VLAN id carries different names on different switches, which makes \
            every later search and every change risky. Default names (`default`, `VLAN0010`) and \
            differences of case, spaces, hyphens and underscores are not reported.",
        not_evaluated: "Fewer than two switches have a name for the VLAN.",
        default_enabled: true,
        run: checks::l2::vlan_name_inconsistent,
    },
    Check {
        id: "mac_flapping",
        title: "MAC addresses moving between two ports",
        category: Category::L2,
        severity: Severity::High,
        needs: &["mac_table", "the previous collection of mac_table"],
        description: "Several MAC addresses in one VLAN moved between the same two ports between \
            two collections, which is what a bridging loop looks like. A single address moving is \
            not reported: that is a laptop changing desks or a wireless client roaming.",
        not_evaluated: "Only one MAC-table collection.",
        default_enabled: true,
        run: checks::l2::mac_flapping,
    },
    // ---------------------------------------------------------------- l3
    Check {
        id: "duplicate_ip",
        title: "Two devices with one address",
        category: Category::L3,
        severity: Severity::Critical,
        needs: &["interfaces", "arp"],
        description: "The same address is configured on interfaces of two devices in the same \
            subnet, or two ARP tables collected minutes apart disagree about which MAC holds it. \
            First-hop redundancy (HSRP/VRRP/GLBP virtual addresses and their virtual MACs) and \
            anycast loopbacks are not duplicates and are excluded.",
        not_evaluated: "A device whose data is older than the staleness threshold; a shared \
            address whose holders do not all have a stored configuration this can read, because a \
            redundancy group is only recognisable from one; or ARP tables collected too far apart \
            to be compared.",
        default_enabled: true,
        run: checks::l3::duplicate_ip,
    },
    Check {
        id: "subnet_mask_mismatch",
        title: "Link ends with different prefix lengths",
        category: Category::L3,
        severity: Severity::High,
        needs: &["interfaces", "neighbors (or the topology feature)"],
        description: "Two interfaces on one link hold overlapping addresses with different prefix \
            lengths, so each thinks a different part of the network is local. Only ends with \
            exactly one address each are compared; secondary addressing is left alone.",
        not_evaluated: "No link, or either end has no address or more than one.",
        default_enabled: true,
        run: checks::l3::subnet_mask_mismatch,
    },
    Check {
        id: "overlapping_subnets",
        title: "Two interfaces of one device in overlapping subnets",
        category: Category::L3,
        severity: Severity::Medium,
        needs: INTERFACES,
        description: "One device holds two interface addresses whose networks overlap without \
            being the same, which makes its own routing ambiguous. Host routes (/32, /31) and \
            loopbacks are excluded.",
        not_evaluated: "Fewer than two addressed interfaces.",
        default_enabled: true,
        run: checks::l3::overlapping_subnets,
    },
    Check {
        id: "no_default_route",
        title: "No default route",
        category: Category::L3,
        severity: Severity::Low,
        needs: &["routes", "running configuration"],
        description: "A device that routes has no 0.0.0.0/0, so anything it has no specific route \
            for is dropped. On a core device this is often deliberate (a default-free core) and is \
            reported as information only. Layer-2 switches, which use `ip default-gateway` rather \
            than a routing table, are excluded.",
        not_evaluated: "No routing table was collected, or the device is a layer-2 switch.",
        default_enabled: true,
        run: checks::l3::no_default_route,
    },
    // ------------------------------------------------------------- config
    Check {
        id: "unsaved_config",
        title: "Running configuration is not saved",
        category: Category::Config,
        severity: Severity::High,
        needs: &["running configuration", "startup configuration"],
        description: "The device would come back after a power cut with a different configuration \
            from the one it is running. Lines that differ only because they are volatile — \
            timestamps, byte counts, the platform's own ignore patterns — do not count.",
        not_evaluated: "Either snapshot is missing, or they were not captured close enough \
            together to be compared (the device may have been saved in between).",
        default_enabled: true,
        run: checks::config::unsaved_config,
    },
    Check {
        id: "no_recent_backup",
        title: "No recent configuration backup",
        category: Category::Hygiene,
        severity: Severity::Medium,
        needs: &["the configuration feature"],
        description: "The newest stored configuration of the device is older than the threshold, \
            or there is none at all — so there is nothing to compare against and nothing to \
            restore from.",
        not_evaluated: "The configuration feature is not installed: nothing is taking backups at \
            all, which is a setup matter and not a finding about a device.",
        default_enabled: true,
        run: checks::config::no_recent_backup,
    },
    Check {
        id: "config_changed_outside_nocturne",
        title: "Configuration changed outside Nocturne",
        category: Category::Config,
        severity: Severity::Medium,
        needs: &["configuration versions", "the changes feature"],
        description: "The running configuration changed between two stored versions, and no \
            Nocturne change was applied to that device around that time. Someone edited the device \
            directly; the diff is in the configuration feature.",
        not_evaluated: "The changes feature is not installed, or the device has fewer than two \
            stored versions.",
        default_enabled: true,
        run: checks::config::config_changed_outside_nocturne,
    },
    // ----------------------------------------------------------- security
    Check {
        id: "telnet_enabled",
        title: "Telnet accepted on the VTY lines",
        category: Category::Security,
        severity: Severity::High,
        needs: &["running configuration"],
        description: "A VTY line accepts telnet, so passwords cross the network in clear text. \
            Read from `transport input` on each `line vty` block; a line that only accepts ssh (or \
            none) is fine.",
        not_evaluated: "No stored configuration, or a configuration grammar this check does not \
            read (it understands IOS-style configurations only).",
        default_enabled: true,
        run: checks::security::telnet_enabled,
    },
    Check {
        id: "http_server_enabled",
        title: "HTTP server enabled",
        category: Category::Security,
        severity: Severity::Medium,
        needs: &["running configuration"],
        description: "`ip http server` is on: an unencrypted management interface, usually left \
            over from the factory defaults. HTTPS (`ip http secure-server`) alone is not reported.",
        not_evaluated: "No stored configuration, or a grammar this check does not read.",
        default_enabled: true,
        run: checks::security::http_server_enabled,
    },
    Check {
        id: "snmp_default_community",
        title: "Default SNMP community",
        category: Category::Security,
        severity: Severity::High,
        needs: &["running configuration"],
        description: "An SNMP community of `public` or `private` is configured — the first thing \
            anything scanning the network will try. A community that is write (`RW`) is worse and \
            is reported as critical.",
        not_evaluated: "No stored configuration, a grammar this check does not read, or the \
            community is encrypted in the stored configuration so its value cannot be seen.",
        default_enabled: true,
        run: checks::security::snmp_default_community,
    },
    Check {
        id: "weak_enable_password",
        title: "Reversible enable password",
        category: Category::Security,
        severity: Severity::High,
        needs: &["running configuration"],
        description: "`enable password` (rather than `enable secret`), or a type 0/5/7 secret: \
            clear text and type 7 are trivially reversible and type 5 is brute-forced offline. \
            Type 8 and 9 are not reported.",
        not_evaluated: "No stored configuration, or a grammar this check does not read.",
        default_enabled: true,
        run: checks::security::weak_enable_password,
    },
    Check {
        id: "no_vty_acl",
        title: "Management access is not restricted",
        category: Category::Security,
        // Low on purpose: on an estate that restricts management with a
        // firewall this is true of every device, and a wall of medium
        // findings is how a list like this stops being read.
        severity: Severity::Low,
        needs: &["running configuration"],
        description: "No `access-class` on the VTY lines, so management is reachable from \
            anywhere that can route to the device.",
        not_evaluated: "No stored configuration, no `line vty` block, or a grammar this check \
            does not read.",
        default_enabled: true,
        run: checks::security::no_vty_acl,
    },
    Check {
        id: "ntp_not_configured",
        title: "No NTP server configured",
        category: Category::Hygiene,
        severity: Severity::Low,
        needs: &["running configuration"],
        description: "Without a time source, every log line and every certificate on the device \
            is suspect, and correlating an incident across devices stops working.",
        not_evaluated: "No stored configuration, or a grammar this check does not read.",
        default_enabled: true,
        run: checks::security::ntp_not_configured,
    },
    Check {
        id: "no_syslog_host",
        title: "No syslog destination",
        category: Category::Hygiene,
        severity: Severity::Low,
        needs: &["running configuration"],
        description: "The device keeps its log to itself, so whatever it says about a fault is \
            lost at the next reboot.",
        not_evaluated: "No stored configuration, or a grammar this check does not read.",
        default_enabled: true,
        run: checks::security::no_syslog_host,
    },
    // ------------------------------------------------------------- fleet
    Check {
        id: "software_inconsistent",
        title: "One model running different software versions",
        category: Category::Hygiene,
        severity: Severity::Low,
        needs: &["facts"],
        description: "Devices of the same model are not on the same software version; the ones in \
            the minority are listed against the majority. Divergent versions mean divergent bugs \
            and divergent syntax.",
        not_evaluated: "Fewer devices of the model than the group threshold, so there is no \
            majority to be in the minority of; or the model or version was not collected.",
        default_enabled: true,
        run: checks::fleet::software_inconsistent,
    },
    Check {
        id: "uptime_reset",
        title: "Device restarted unexpectedly",
        category: Category::Physical,
        severity: Severity::Medium,
        needs: &["facts", "the previous collection of facts"],
        description: "Uptime went backwards between two collections, and the new uptime is short \
            enough that the restart happened between them. A counter that merely wrapped, or \
            uptime that went backwards by more than the time that passed, is not read as a reboot.",
        not_evaluated: "Only one collection of facts, or either one has no uptime.",
        default_enabled: true,
        run: checks::fleet::uptime_reset,
    },
    Check {
        id: "high_cpu",
        title: "CPU sustained high",
        category: Category::Capacity,
        severity: Severity::Medium,
        needs: &["`cpu.pct` samples (monitoring)"],
        description: "The processor stayed above the threshold for a long stretch, not for one \
            poll. A single spike while the device answers a `show tech` is not a finding.",
        not_evaluated: "No CPU samples for the device, or too short a history.",
        default_enabled: true,
        run: checks::fleet::high_cpu,
    },
    Check {
        id: "high_memory",
        title: "Memory sustained high",
        category: Category::Capacity,
        severity: Severity::Medium,
        needs: &["`mem.pct` samples (monitoring)"],
        description: "Memory use stayed above the threshold. On most platforms this climbs and \
            never falls back until the device is restarted.",
        not_evaluated: "No memory samples for the device, or too short a history.",
        default_enabled: true,
        run: checks::fleet::high_memory,
    },
    Check {
        id: "high_temperature",
        title: "Temperature sustained high",
        category: Category::Capacity,
        severity: Severity::High,
        needs: &["`temp.c` samples (monitoring)"],
        description: "A sensor stayed above the threshold — a blocked intake, a dead fan, or a \
            cupboard that is not a comms room.",
        not_evaluated: "No temperature samples for the device, or too short a history.",
        default_enabled: true,
        run: checks::fleet::high_temperature,
    },
    // --------------------------------------------------------------- qos
    Check {
        id: "qos_policy_missing",
        title: "Service policy names a policy-map that is not defined",
        category: Category::Config,
        severity: Severity::Medium,
        needs: &["running configuration"],
        description: "An interface, or a class of another policy, references a policy-map that \
            the same configuration does not define, so the QoS it was meant to apply is not \
            there. Reads IOS-style MQC only: `service-policy input|output` under an interface \
            and `service-policy` inside a class, against `policy-map` sections. It cannot see a \
            policy the platform supplies without printing it: references from `control-plane`, \
            templates and wireless profiles are not judged, and AutoQoS names are never called \
            missing. A device normally refuses such a line, so on hardware a finding usually \
            means the stored configuration is not the whole of what the device runs.",
        not_evaluated: "No stored configuration; one in a grammar this does not read (IOS-style \
            only) — typed QoS policies (`policy-map type qos`, as NX-OS and EOS write them) or \
            an IOS XR configuration (`end-policy-map`, `end-policy`); or one that is not whole — \
            an interface-only or section capture (no `hostname` line) has attachments and no \
            definitions, and a listing that stops before its closing `end` is missing whatever \
            it did not print.",
        default_enabled: true,
        run: checks::qos::qos_policy_missing,
    },
    Check {
        id: "qos_policy_unattached",
        title: "Policy-map attached nowhere",
        category: Category::Hygiene,
        // Info on purpose: estates that push one template to every router
        // define a policy per circuit speed and attach the one that fits, so
        // this is true of most of their devices.
        severity: Severity::Info,
        needs: &["running configuration"],
        description: "A policy-map that no `service-policy` line anywhere in the configuration \
            names: left over, or written and never attached — in which case the QoS it describes \
            is not in effect. A child policy referenced from a class counts as used, as does any \
            other mention after `service-policy` (control-plane, a template, an NHRP group). \
            Policies the platform generates (`AutoQos-…`, `system-cpp-policy`) are not reported. \
            It cannot see a policy that RADIUS or a controller applies by name; IOS-style MQC \
            only.",
        not_evaluated: "No stored configuration; one in a grammar this does not read (IOS-style \
            only) — typed QoS policies or an IOS XR configuration; one that is not whole (no \
            `hostname` line, or no closing `end`: IOS prints policy-maps before interfaces, so a \
            listing cut short has the definitions without the attachments); or a device whose \
            sessions RADIUS authorises (a virtual template, a dynamic template, `ip subscriber`, \
            `aaa authorization network`) and can give a policy by name.",
        default_enabled: true,
        run: checks::qos::qos_policy_unattached,
    },
    Check {
        id: "qos_class_dropping",
        title: "QoS class dropping packets",
        category: Category::Capacity,
        severity: Severity::Medium,
        needs: &[
            "`qos.queue_drop_ps` samples (monitoring)",
            "`qos.drop_ps` samples where the device has no separate congestion counter",
        ],
        description: "A class of a QoS policy kept dropping packets for most of a long stretch \
            that runs up to now, with its drop rate, the share of offered packets it lost and \
            the hour it is worst in. Where the device counts congestion (queue) drops apart, \
            only those count: a policer that drops is the policy working, and a class with no \
            queue is not reported. Where a device records no congestion counter — one drop \
            counter for both, or no class with a queue — the finding says so and is low \
            severity. Only samples since the last hole in a series are judged, so a read that \
            failed for a while is not counted as time spent dropping. On a hierarchical policy a \
            parent class counts its children's drops, so a congested child can appear together \
            with its parent.",
        not_evaluated: "No QoS samples for the device — its device type has no QoS read, \
            monitoring is not polling it, the read has never succeeded, or no policy is attached \
            — none inside the window, or samples that stop before now because the read is \
            failing; and a drop threshold that is not above zero.",
        default_enabled: true,
        run: checks::qos::qos_class_dropping,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalogue_is_consistent() {
        let mut ids: Vec<&str> = CHECKS.iter().map(|check| check.id).collect();
        ids.sort_unstable();
        let unique = {
            let mut unique = ids.clone();
            unique.dedup();
            unique
        };
        assert_eq!(ids, unique, "two checks share an id");
        for check in CHECKS {
            assert!(!check.needs.is_empty(), "{} declares no data", check.id);
            assert!(
                check.description.len() > 60,
                "{} needs a description a person can judge it by",
                check.id
            );
            assert!(
                check.not_evaluated.len() > 20,
                "{} must say when it declines to answer",
                check.id
            );
            assert!(
                check.id.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{} is not a snake_case id",
                check.id
            );
        }
        assert!(get("duplex_mismatch").is_some());
        assert!(get("nope").is_none());
    }
}

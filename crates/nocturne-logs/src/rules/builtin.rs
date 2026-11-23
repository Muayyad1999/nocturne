//! The rules that ship with the application.
//!
//! Each is written for one platform's messages and says so (`platform`):
//! Cisco IOS / IOS-XE and MikroTik RouterOS, and nothing else. A device of
//! another platform gets no built-in syslog rule, which is a gap the rule
//! list shows rather than hides.
//!
//! The rules on the `trap` stream are not a vendor's: they match the
//! notifications SNMPv2-MIB and IF-MIB define, which any agent may send,
//! and they read the message the trap receiver writes (`traps::render`)
//! rather than text a device chose.
//!
//! Every rule carries a line it is meant to match and where that line came
//! from. "Quoted from" means the line stands in the vendor's documentation
//! as written here; "Synthetic" means the documentation gives the message's
//! format and the values were filled in by hand. Neither is a line recorded
//! from a device of ours. The oldest rules name the vendor's documentation
//! without a page, which is all that is known about where their lines came
//! from. A trap rule's line is the message this application wrote for a
//! notification net-snmp sent, and says so.
//!
//! A new group of rules is one more function beside [`ios`], [`routeros`]
//! and [`traps`], chained into [`builtins`] with its own platform label
//! and, where it reads another stream, its `stream`.

use crate::model::{LogRule, Signal, Threshold};

/// The platforms the built-in rules cover, as the rule list shows them.
pub const IOS: &str = "Cisco IOS / IOS-XE";
pub const ROUTEROS: &str = "MikroTik RouterOS";
/// Not a platform: the notifications the standard MIBs define, whoever
/// sends them.
pub const SNMP: &str = "Any SNMP agent (SNMPv2-MIB, IF-MIB)";

const IOS_DOC: &str = "Cisco IOS / IOS-XE system message documentation";
const ROS_DOC: &str = "MikroTik RouterOS logging documentation";

// A login line carries one field its sender does not write: the user name,
// which is whatever was typed at the prompt. Someone who types
// `x] [Source: 192.0.2.9` — or `root from 192.0.2.9 via ssh` — as a name
// would, with a pattern that stops at the first bracket or the first
// ` from `, be counted under an address of their choosing: a different one
// per attempt, and the burst never adds up. So the name is taken greedily
// and the address is the last one in the line, where the device wrote it.

/// IOS `Login failed [user: …] [Source: …] [localport: …] [Reason: …] at …`.
const IOS_LOGIN_FAILED: &str = r"%SEC_LOGIN-4-LOGIN_FAILED: ?Login failed \[user: ?(?P<user>.*)\] \[Source: ?(?P<source>[^\]]*)\] \[localport: ?\d+\] \[Reason:";
/// IOS `Login Success [user: …] [Source: …] [localport: …] at …`.
const IOS_LOGIN_SUCCESS: &str = r"%SEC_LOGIN-5-LOGIN_SUCCESS: ?Login Success \[user: ?(?P<user>.*)\] \[Source: ?(?P<source>[^\]]*)\] \[localport: ?\d+\] at ";
/// RouterOS `login failure for user … from … via …`, which ends the line.
const ROS_LOGIN_FAILURE: &str =
    r"login failure for user (?P<user>.*) from (?P<source>\S+) via (?P<service>\S+)\s*$";

fn rule(
    id: &str,
    name: &str,
    pattern: &str,
    emit: Signal,
    test_line: &str,
    source: &str,
) -> LogRule {
    LogRule {
        id: id.to_string(),
        name: name.to_string(),
        builtin: true,
        pattern: pattern.to_string(),
        emit,
        test_line: test_line.to_string(),
        test_line_source: source.to_string(),
        ..LogRule::default()
    }
}

/// Where the trap rules' lines come from. They are not a device's words in
/// any case — a trap is numbers, and the words are this application's.
fn trap_line(sent: &str) -> String {
    format!(
        "Synthetic: the message this application's trap receiver writes for the SNMPv2c \
         notification that net-snmp 5.9.4's `snmptrap` sends for `{sent}` (OIDs given as numbers). \
         That datagram was captured and is replayed in this crate's tests/traps.rs, which checks \
         that the message is this one. net-snmp sent it, not a device, and its values were \
         chosen by hand"
    )
}

fn alert(severity: &str, title: &str) -> Signal {
    Signal::Alert {
        severity: severity.to_string(),
        title: title.to_string(),
    }
}

/// The parts of a rule that only some of them have, written so the list
/// below reads as a description of each rule.
trait Tuned {
    /// One instance per value of these captures (a neighbour, an address)
    /// rather than one per device: each line still counts on its own.
    fn per(self, group_by: &[&str]) -> Self;
    /// Raise only when `count` lines arrive inside `window_s` for one
    /// device and one value of these captures.
    fn at_least(self, count: u32, window_s: u32, group_by: &[&str]) -> Self;
    /// The opposite event: a line matching this clears the alert.
    fn cleared_by(self, pattern: &str) -> Self;
    /// Clear the alert after this long without a matching line.
    fn cleared_after(self, seconds: u32) -> Self;
    /// Only while this other rule's alert is open for the same instance.
    fn after(self, rule_id: &str) -> Self;
}

impl Tuned for LogRule {
    fn per(self, group_by: &[&str]) -> Self {
        // The window means nothing when one line is enough; it is the
        // throttle window so the rule reads the same either way.
        let window_s = self.throttle_s;
        self.at_least(1, window_s, group_by)
    }

    fn at_least(mut self, count: u32, window_s: u32, group_by: &[&str]) -> Self {
        self.threshold = Some(Threshold {
            count,
            window_s,
            group_by: group_by.iter().map(|name| name.to_string()).collect(),
        });
        self
    }

    fn cleared_by(mut self, pattern: &str) -> Self {
        self.clear_pattern = Some(pattern.to_string());
        self
    }

    fn cleared_after(mut self, seconds: u32) -> Self {
        self.clear_after_s = Some(seconds);
        self
    }

    fn after(mut self, rule_id: &str) -> Self {
        self.after = Some(rule_id.to_string());
        self
    }
}

/// Every built-in rule, each labelled with the platform it is for.
///
/// Their scope is deliberately empty: the pattern itself names the vendor's
/// mnemonic, so a rule written for IOS cannot match a RouterOS line, and
/// scoping by profile would only stop the rule working on a device whose
/// profile has not been detected yet.
pub fn builtins() -> Vec<LogRule> {
    let labelled = |platform: &'static str| {
        move |rule: LogRule| LogRule {
            platform: platform.to_string(),
            ..rule
        }
    };
    ios()
        .into_iter()
        .map(labelled(IOS))
        .chain(routeros().into_iter().map(labelled(ROUTEROS)))
        .chain(traps().into_iter().map(labelled(SNMP)).map(|rule| LogRule {
            stream: crate::db::TRAP_STREAM.to_string(),
            ..rule
        }))
        .collect()
}

/// The standard notifications. Each pattern starts at the notification's
/// name, which is the first word of the message the receiver writes.
fn traps() -> Vec<LogRule> {
    vec![
        // Keyed by the number, which is in every such trap, and not by the
        // name, which is only there when the monitor has polled the device:
        // a map that arrives between the down and the up must not make them
        // two different ports. `name` is ` (GigabitEthernet0/3)` or nothing.
        rule(
            "builtin.trap.link_down",
            "Trap: link down",
            r"^linkDown\b.*?\bifIndex=(?P<ifindex>\d+)(?P<name>(?: \([^)]*\))?)",
            alert("low", "Link down: ifIndex {ifindex}{name}"),
            "linkDown ifIndex=3 ifAdminStatus=up(1) ifOperStatus=down(2)",
            &trap_line("linkDown ifIndex.3 i 3 ifAdminStatus.3 i 1 ifOperStatus.3 i 2"),
        )
        .per(&["ifindex"])
        .cleared_by(r"^linkUp\b.*?\bifIndex=(?P<ifindex>\d+)"),
        rule(
            "builtin.trap.cold_start",
            "Trap: device restarted (coldStart)",
            r"^coldStart\b",
            Signal::Reload,
            "coldStart",
            &trap_line("coldStart"),
        ),
        rule(
            "builtin.trap.warm_start",
            "Trap: SNMP agent restarted (warmStart)",
            r"^warmStart\b",
            Signal::Reload,
            "warmStart",
            &trap_line("warmStart"),
        ),
        // One failure is a mistyped community on somebody's laptop. Eight
        // in a minute is something walking through a list of them.
        rule(
            "builtin.trap.auth_failure_burst",
            "Trap: repeated SNMP authentication failures",
            r"^authenticationFailure\b",
            alert("high", "Repeated SNMP authentication failures"),
            "authenticationFailure",
            &trap_line("authenticationFailure"),
        )
        .at_least(8, 60, &[])
        .cleared_after(600),
    ]
}

fn ios() -> Vec<LogRule> {
    vec![
        rule(
            "builtin.ios.config_changed",
            "IOS: configuration changed",
            r"%SYS-5-CONFIG_I: Configured from (?P<from>\S+)(?: by (?P<by>\S+))?",
            Signal::ConfigChanged,
            "*Mar  1 00:12:02.123: %SYS-5-CONFIG_I: Configured from console by admin on vty0 (10.0.0.5)",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.link_down",
            "IOS: interface down",
            r"%LINK-3-UPDOWN: Interface (?P<interface>[\w./:-]+), changed state to (?:administratively )?down",
            Signal::InterfaceDown {
                interface_group: "interface".into(),
            },
            "*Mar  1 00:12:10.001: %LINK-3-UPDOWN: Interface GigabitEthernet1/0/3, changed state to down",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.link_up",
            "IOS: interface up",
            r"%LINK-3-UPDOWN: Interface (?P<interface>[\w./:-]+), changed state to up",
            Signal::InterfaceUp {
                interface_group: "interface".into(),
            },
            "*Mar  1 00:12:14.001: %LINK-3-UPDOWN: Interface GigabitEthernet1/0/3, changed state to up",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.lineproto_down",
            "IOS: line protocol down",
            r"%LINEPROTO-5-UPDOWN: Line protocol on Interface (?P<interface>[\w./:-]+), changed state to down",
            Signal::InterfaceDown {
                interface_group: "interface".into(),
            },
            "*Mar  1 00:12:11.002: %LINEPROTO-5-UPDOWN: Line protocol on Interface GigabitEthernet1/0/3, changed state to down",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.lineproto_up",
            "IOS: line protocol up",
            r"%LINEPROTO-5-UPDOWN: Line protocol on Interface (?P<interface>[\w./:-]+), changed state to up",
            Signal::InterfaceUp {
                interface_group: "interface".into(),
            },
            "*Mar  1 00:12:15.002: %LINEPROTO-5-UPDOWN: Line protocol on Interface GigabitEthernet1/0/3, changed state to up",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.login_failed",
            "IOS: login failure",
            IOS_LOGIN_FAILED,
            Signal::LoginFailure,
            "*Mar  1 00:13:01.000: %SEC_LOGIN-4-LOGIN_FAILED: Login failed [user: admin] [Source: 10.0.0.5] [localport: 22] [Reason: Login Authentication Failed] at 00:13:01 UTC Mon Mar 1 2027",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.restart",
            "IOS: device restarted",
            r"%SYS-5-RESTART: System restarted",
            Signal::Reload,
            "*Mar  1 00:00:07.123: %SYS-5-RESTART: System restarted --\nCisco IOS Software, C2960X Software",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.spantree",
            "IOS: spanning tree event",
            r"%SPANTREE-2-(?P<event>[A-Z_0-9]+): (?P<detail>.*)",
            alert("high", "Spanning tree: {event}"),
            "*Mar  1 00:14:00.000: %SPANTREE-2-ROOTGUARD_BLOCK: Root guard blocking port GigabitEthernet1/0/5 on VLAN0010.",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.native_vlan_mismatch",
            "IOS: native VLAN mismatch",
            r"%CDP-4-NATIVE_VLAN_MISMATCH: Native VLAN mismatch discovered on (?P<interface>[\w./:-]+) \((?P<local>\d+)\), with (?P<neighbor>\S+) (?P<remote_interface>[\w./:-]+) \((?P<remote>\d+)\)",
            alert(
                "medium",
                "Native VLAN mismatch on {interface}: {local} vs {remote}",
            ),
            "*Mar  1 00:15:00.000: %CDP-4-NATIVE_VLAN_MISMATCH: Native VLAN mismatch discovered on GigabitEthernet1/0/2 (1), with SW2 GigabitEthernet1/0/2 (10).",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.duplex_mismatch",
            "IOS: duplex mismatch",
            r"%CDP-4-DUPLEX_MISMATCH: duplex mismatch discovered on (?P<interface>[\w./:-]+)",
            alert("medium", "Duplex mismatch on {interface}"),
            "*Mar  1 00:15:30.000: %CDP-4-DUPLEX_MISMATCH: duplex mismatch discovered on GigabitEthernet1/0/4 (not half duplex), with SW2 GigabitEthernet0/2 (half duplex).",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.err_disable",
            "IOS: port error-disabled",
            r"%PM-4-ERR_DISABLE: (?P<reason>[\w-]+) error detected on (?P<interface>[\w./:-]+), putting",
            alert("high", "{interface} error-disabled ({reason})"),
            "*Mar  1 00:16:00.000: %PM-4-ERR_DISABLE: bpduguard error detected on Gi1/0/7, putting Gi1/0/7 in err-disable state",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.eigrp_neighbor",
            "IOS: EIGRP neighbour change",
            r"%DUAL-5-NBRCHANGE:.*Neighbor (?P<neighbor>\S+) \((?P<interface>[^)]+)\) is down",
            alert("medium", "EIGRP neighbour {neighbor} is down on {interface}"),
            "*Mar  1 00:17:00.000: %DUAL-5-NBRCHANGE: EIGRP-IPv4 100: Neighbor 10.0.0.2 (GigabitEthernet1/0/1) is down: holding time expired",
            IOS_DOC,
        )
        .per(&["neighbor", "interface"])
        .cleared_by(
            r"%DUAL-5-NBRCHANGE:.*Neighbor (?P<neighbor>\S+) \((?P<interface>[^)]+)\) is up",
        ),
        rule(
            "builtin.ios.ospf_adjacency",
            "IOS: OSPF adjacency change",
            r"%OSPF-5-ADJCHG: Process \S+, Nbr (?P<neighbor>\S+) on (?P<interface>[\w./:-]+) from (?P<from>FULL) to (?P<state>[\w-]+)",
            alert(
                "medium",
                "OSPF neighbour {neighbor} on {interface} went {from} → {state}",
            ),
            "*Mar  1 00:18:00.000: %OSPF-5-ADJCHG: Process 1, Nbr 10.0.0.2 on GigabitEthernet1/0/1 from FULL to DOWN, Neighbor Down: Dead timer expired",
            IOS_DOC,
        )
        .per(&["neighbor", "interface"])
        .cleared_by(
            r"%OSPF-5-ADJCHG: Process \S+, Nbr (?P<neighbor>\S+) on (?P<interface>[\w./:-]+) from \S+ to FULL\b",
        ),
        rule(
            "builtin.ios.bgp_adjacency",
            "IOS: BGP neighbour change",
            r"%BGP-5-ADJCHANGE: neighbor (?P<neighbor>\S+)(?: \S+)*? Down\b",
            alert("high", "BGP neighbour {neighbor} is down"),
            "*Mar  1 00:19:00.000: %BGP-5-ADJCHANGE: neighbor 192.0.2.1 Down BGP Notification sent",
            IOS_DOC,
        )
        .per(&["neighbor"])
        .cleared_by(r"%BGP-5-ADJCHANGE: neighbor (?P<neighbor>\S+)(?: \S+)*? Up\b"),
        rule(
            "builtin.ios.mac_flap",
            "IOS: MAC address flapping",
            r"%SW_MATM-4-MACFLAP_NOTIF: Host (?P<mac>\S+) in vlan (?P<vlan>\d+) is flapping between port (?P<interface>[\w./:-]+) and port (?P<other>[\w./:-]+)",
            alert(
                "high",
                "{mac} is flapping between {interface} and {other} in VLAN {vlan}",
            ),
            "*Mar  1 00:20:00.000: %SW_MATM-4-MACFLAP_NOTIF: Host 0011.2233.4455 in vlan 10 is flapping between port Gi1/0/3 and port Gi1/0/9",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.environment",
            "IOS: environment alarm",
            r"%(?:ENVMON|PLATFORM_ENV|CISCO_ENVMON|ENVIRONMENT)-[0-4]-(?P<event>[A-Z_0-9]+): (?P<detail>.*)",
            alert("critical", "Environment: {event}"),
            "*Mar  1 00:21:00.000: %PLATFORM_ENV-1-FAN_FAILED: Fan 2 not rotating.",
            IOS_DOC,
        ),
        // ------------------------------------------------------ security
        rule(
            "builtin.ios.login_burst",
            "IOS: repeated failed logins from one address",
            IOS_LOGIN_FAILED,
            // The user name is left out of the title on purpose: people
            // type passwords into that field, and a title is sent to
            // notification channels.
            alert("high", "Repeated failed logins from {source}"),
            "%SEC_LOGIN-4-LOGIN_FAILED: Login failed [user: admin] [Source: 10.4.2.11] [localport: 22] [Reason: Login Authentication Failed] at 20:54:42 UTC Fri Feb 28 2003",
            "Synthetic: written to the format `Login failed [user: [chars]] [Source: [chars]] [localport: [dec]] [Reason: [chars]] at [chars]` that Cisco's System Message Guide for IOS XE 16.12.x gives for SEC_LOGIN-4-LOGIN_FAILED (https://www.cisco.com/c/dam/en/us/td/docs/switches/lan/sys_messages/16-12/smg_1612x.xlsx); the values are filled in by hand, it is not a line a device sent",
        )
        .at_least(8, 60, &["source"])
        .cleared_after(600),
        rule(
            "builtin.ios.login_after_burst",
            "IOS: login succeeded after repeated failures",
            IOS_LOGIN_SUCCESS,
            alert(
                "critical",
                "A login from {source} succeeded straight after repeated failures",
            ),
            "*Aug 22 06:06:02.617: %SEC_LOGIN-5-LOGIN_SUCCESS: Login Success [user: ciscoadmin] [Source: 10.189.3.9] [localport: 22] at 06:06:02 UTC Fri Aug 22 2014",
            "Quoted from Cisco, \"Configure SSH on Routers\", Document ID 4145, debug output of a successful login (https://www.cisco.com/c/en/us/support/docs/security-vpn/secure-shell-ssh/4145-ssh.html)",
        )
        .per(&["source"])
        .after("builtin.ios.login_burst"),
        rule(
            "builtin.ios.port_security",
            "IOS: port-security violation",
            r"%PORT_SECURITY-2-PSECURE_VIOLATION: Security violation occurred, caused by MAC address (?P<mac>[0-9A-Fa-f.:-]+) on port (?P<interface>[A-Za-z][\w/:-]*(?:\.\d+)?)",
            alert("high", "Port-security violation on {interface} by {mac}"),
            "%PORT_SECURITY-2-PSECURE_VIOLATION: Security violation occurred, caused by MAC address 001b.77ee.eeee on port GigabitEthernet1/0/18.",
            "Quoted from Cisco, \"Layer 2 Security Features on Cisco Catalyst Layer 3 Fixed Configuration Switches Configuration Example\", Document ID 72846, where the page breaks the line in two after `occurred,` (https://www.cisco.com/c/en/us/support/docs/switches/catalyst-3750-series-switches/72846-layer2-secftrs-catl3fixed.html)",
        )
        .cleared_after(1_800),
        rule(
            "builtin.ios.dhcp_snooping",
            "IOS: DHCP snooping dropped a message",
            r"%DHCP_SNOOPING-5-DHCP_SNOOPING_(?P<event>UNTRUSTED_PORT|MATCH_MAC_FAIL|NONZERO_GIADDR|FAKE_INTERFACE): (?P<detail>.*)",
            alert("medium", "DHCP snooping dropped a message ({event})"),
            "%DHCP_SNOOPING-5-DHCP_SNOOPING_UNTRUSTED_PORT: DHCP_SNOOPING drop message on untrusted port, message type: DHCPOFFER, MAC sa: 5c5a.c7d6.9e1f",
            "Quoted from Cisco, \"Operate and Troubleshoot DHCP Snooping on Catalyst 9000 Switches\", Document ID 217055 (https://www.cisco.com/c/en/us/support/docs/ip/dynamic-host-configuration-protocol-dhcp-dhcpv6/217055-operate-and-troubleshoot-dhcp-snooping.html)",
        )
        .per(&["event"])
        .cleared_after(1_800),
        rule(
            "builtin.ios.arp_inspection",
            "IOS: dynamic ARP inspection dropped packets",
            r"%SW_DAI-4-(?P<event>DHCP_SNOOPING_DENY|ACL_DENY|INVALID_ARP): (?P<packets>\d+) Invalid ARPs (?:\(?(?P<type>\w+)\)? )?on (?P<interface>[A-Za-z][\w/:-]*(?:\.\d+)?),? vlan (?P<vlan>\d+)",
            alert(
                "high",
                "Dynamic ARP inspection dropped ARP packets on {interface} (VLAN {vlan})",
            ),
            "%SW_DAI-4-DHCP_SNOOPING_DENY: 1 Invalid ARPs (Req) on Gi1/0/2, vlan 10.([0011.2233.4455/10.0.0.3/0000.0000.0000/10.0.0.1/02:46:49 UTC Mon Mar 1 2027])",
            "Synthetic: written to the format `%ld Invalid ARPs [chars] on [chars] vlan [dec].[[enet]/[chars]/[enet]/[chars]/[clock]]` that Cisco's System Message Guide for IOS XE 16.12.x gives for SW_DAI-4-DHCP_SNOOPING_DENY (https://www.cisco.com/c/dam/en/us/td/docs/switches/lan/sys_messages/16-12/smg_1612x.xlsx); the values and the punctuation the spreadsheet drops are filled in by hand, it is not a line a device sent",
        )
        .cleared_after(1_800),
        rule(
            "builtin.ios.flap_storm",
            "IOS: interface flapping",
            r"%LINK-3-UPDOWN: Interface (?P<interface>[\w./:-]+), changed state to down",
            alert("high", "{interface} is flapping: it keeps going down"),
            "*Aug 25 20:01:07.792: %LINK-3-UPDOWN: Interface TenGigabitEthernet1/0/2, changed state to down",
            "Quoted from Cisco, \"Troubleshoot Port Flaps on Catalyst 9000 Switches\", Document ID 218397, the log around a port flap (https://www.cisco.com/c/en/us/support/docs/switches/catalyst-9500-series-switches/218397-troubleshoot-port-flaps-on-catalyst-9000.html)",
        )
        .at_least(5, 300, &["interface"])
        .cleared_after(900),
    ]
}

fn routeros() -> Vec<LogRule> {
    vec![
        rule(
            "builtin.routeros.config_changed",
            "RouterOS: configuration changed",
            r"system,info,?\S* .*changed by (?P<by>\S+)",
            Signal::ConfigChanged,
            "system,info,account ip address changed by admin",
            ROS_DOC,
        ),
        rule(
            "builtin.routeros.login_failed",
            "RouterOS: login failure",
            ROS_LOGIN_FAILURE,
            Signal::LoginFailure,
            "system,error,critical login failure for user admin from 10.0.0.5 via ssh",
            ROS_DOC,
        ),
        rule(
            "builtin.routeros.link_down",
            "RouterOS: interface down",
            r"interface,info (?P<interface>[\w.@/-]+) link down",
            Signal::InterfaceDown {
                interface_group: "interface".into(),
            },
            "interface,info ether3 link down",
            ROS_DOC,
        ),
        rule(
            "builtin.routeros.link_up",
            "RouterOS: interface up",
            r"interface,info (?P<interface>[\w.@/-]+) link up",
            Signal::InterfaceUp {
                interface_group: "interface".into(),
            },
            "interface,info ether3 link up (speed 1G, full duplex)",
            ROS_DOC,
        ),
        rule(
            "builtin.routeros.ospf_state",
            "RouterOS: OSPF neighbour state change",
            r"ospf\S*,?\S* .*(?:neighbor|neighbour) (?P<neighbor>\S+?):? state change from (?P<from>Full) to (?P<state>[\w-]+)",
            alert("medium", "OSPF neighbour {neighbor} went {from} → {state}"),
            "route,ospf,info OSPFv2 neighbor 10.0.0.2: state change from Full to Down",
            ROS_DOC,
        )
        .per(&["neighbor"])
        .cleared_by(
            r"ospf\S*,?\S* .*(?:neighbor|neighbour) (?P<neighbor>\S+?):? state change from \S+ to Full\b",
        ),
        // ------------------------------------------------------ security
        rule(
            "builtin.routeros.login_burst",
            "RouterOS: repeated failed logins from one address",
            ROS_LOGIN_FAILURE,
            alert("high", "Repeated failed logins from {source}"),
            "system,error,critical login failure for user root from 10.0.0.1 via telnet",
            "Quoted from MikroTik, RouterOS documentation, \"Command Line Interface\", the login process, without the `dec/10/2007 10:40:06` the console prints in front of the line there and a device does not send (https://help.mikrotik.com/docs/spaces/ROS/pages/328134/Command+Line+Interface)",
        )
        .at_least(8, 60, &["source"])
        .cleared_after(600),
        rule(
            "builtin.routeros.flap_storm",
            "RouterOS: interface flapping",
            r"interface,info (?P<interface>[\w.@/-]+) link down",
            alert("high", "{interface} is flapping: it keeps going down"),
            "interface,info ether1 link down",
            "Synthetic: written to the message format `%{DATA:network.name} link down` in MikroTik's RouterOS documentation, \"Syslog with Elasticsearch\" (https://help.mikrotik.com/docs/spaces/ROS/pages/282132697/Syslog+with+Elasticsearch). That page gives the message and not its topics: the `interface,info` in front and the interface name are filled in by hand, and it is not a line a device sent",
        )
        .at_least(5, 300, &["interface"])
        .cleared_after(900),
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use regex::Regex;

    use super::*;
    use crate::rules::{check, groups_of};

    /// A rule that counts lines another rule reports one by one. The two
    /// are meant to match the same line: one says "a port went down", the
    /// other says "that is the fifth time in five minutes".
    const COUNTS_THE_LINES_OF: &[(&str, &str)] = &[
        ("builtin.ios.flap_storm", "builtin.ios.link_down"),
        ("builtin.ios.login_burst", "builtin.ios.login_failed"),
        ("builtin.routeros.flap_storm", "builtin.routeros.link_down"),
        (
            "builtin.routeros.login_burst",
            "builtin.routeros.login_failed",
        ),
    ];

    #[test]
    fn every_built_in_rule_matches_its_own_test_line() {
        for rule in builtins() {
            let pattern = Regex::new(&rule.pattern)
                .unwrap_or_else(|err| panic!("{} does not compile: {err}", rule.id));
            assert!(
                pattern.is_match(&rule.test_line),
                "{} does not match the line it ships with:\n  {}",
                rule.id,
                rule.test_line
            );
            assert!(
                !rule.test_line_source.is_empty(),
                "{} does not say where its test line came from",
                rule.id
            );
            if let Signal::InterfaceDown { interface_group }
            | Signal::InterfaceUp { interface_group } = &rule.emit
            {
                let groups = groups_of(&pattern, &rule.test_line).expect("it matched");
                assert!(
                    groups.contains_key(interface_group),
                    "{} promises an interface in `{interface_group}` and captures {groups:?}",
                    rule.id
                );
            }
        }
    }

    #[test]
    fn a_test_line_says_whether_it_is_quoted_or_written_by_hand() {
        // The rules added with the security set cite a page. A line that
        // was built from a documented format must not pass for a quotation.
        for rule in builtins() {
            let source = &rule.test_line_source;
            if !source.contains("https://") {
                continue;
            }
            assert!(
                source.starts_with("Quoted from ") || source.starts_with("Synthetic: "),
                "{} cites a page without saying whether the line is quoted from it: {source}",
                rule.id
            );
            assert!(
                source.contains("www.cisco.com") || source.contains("help.mikrotik.com"),
                "{} cites something that is not the vendor's own documentation: {source}",
                rule.id
            );
        }
        let cited = builtins()
            .iter()
            .filter(|rule| rule.test_line_source.contains("https://"))
            .count();
        assert!(
            cited >= 8,
            "the security rules lost their citations: {cited}"
        );
    }

    #[test]
    fn every_built_in_rule_says_which_platform_it_is_for() {
        let rules = builtins();
        for rule in &rules {
            assert!(
                [IOS, ROUTEROS, SNMP].contains(&rule.platform.as_str()),
                "{} is for `{}`, which is not a platform the built-ins cover",
                rule.id,
                rule.platform
            );
            let prefix = match rule.platform.as_str() {
                IOS => "builtin.ios.",
                ROUTEROS => "builtin.routeros.",
                _ => "builtin.trap.",
            };
            assert_eq!(
                rule.stream == crate::db::TRAP_STREAM,
                rule.platform == SNMP,
                "{} reads `{}` and is labelled {}",
                rule.id,
                rule.stream,
                rule.platform
            );
            assert!(
                rule.id.starts_with(prefix),
                "{} is labelled {}",
                rule.id,
                rule.platform
            );
        }
        let ids: HashSet<&str> = rules.iter().map(|rule| rule.id.as_str()).collect();
        assert_eq!(ids.len(), rules.len(), "two built-in rules share an id");
    }

    #[test]
    fn built_in_rules_pass_what_a_saved_rule_has_to_pass() {
        let rules = builtins();
        for rule in &rules {
            let other = |id: &str| rules.iter().find(|other| other.id == id).cloned();
            check(rule, &other).unwrap_or_else(|err| panic!("{}: {err}", rule.id));
        }
    }

    #[test]
    fn the_opposite_of_a_rule_is_not_also_its_match() {
        let mut clearing = 0;
        for rule in builtins() {
            let Some(clear) = &rule.clear_pattern else {
                continue;
            };
            clearing += 1;
            let clear = Regex::new(clear).expect("checked above");
            assert!(
                !clear.is_match(&rule.test_line),
                "{} would clear on the very line that raises it",
                rule.id
            );
        }
        assert!(clearing >= 4, "the neighbour rules lost their opposites");
    }

    #[test]
    fn rules_do_not_match_each_others_lines() {
        let rules = builtins();
        for rule in &rules {
            let pattern = Regex::new(&rule.pattern).expect("compiles");
            for other in &rules {
                if other.id == rule.id {
                    continue;
                }
                // Up and down share a mnemonic; everything else must be
                // distinguishable or a single event raises two signals.
                let pair = rule.id.replace("_up", "_down") == other.id.replace("_up", "_down");
                if pair {
                    continue;
                }
                let counted = COUNTS_THE_LINES_OF.iter().any(|(counting, counted)| {
                    (*counting == rule.id && *counted == other.id)
                        || (*counting == other.id && *counted == rule.id)
                });
                if counted {
                    continue;
                }
                assert!(
                    !pattern.is_match(&other.test_line),
                    "{} also matches the test line of {}",
                    rule.id,
                    other.id
                );
            }
        }
        // The exemption is only for a rule that really counts: one that
        // raised on every line would double each event.
        for (counting, _) in COUNTS_THE_LINES_OF {
            let rule = rules
                .iter()
                .find(|rule| rule.id == *counting)
                .unwrap_or_else(|| panic!("{counting} is not a built-in rule"));
            assert!(
                rule.threshold
                    .as_ref()
                    .is_some_and(|threshold| threshold.count > 1),
                "{counting} is exempt as a counting rule and does not count"
            );
        }
    }
}

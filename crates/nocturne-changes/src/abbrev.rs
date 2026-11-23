//! Abbreviations, and why they are refused.
//!
//! A Cisco-style CLI accepts any unambiguous abbreviation: `int gi0/1`,
//! `sw tr al v 10`, `no shut`. A planner that only recognises full keywords
//! would read those lines as something it does not know, under-state their
//! risk, and fail to notice that `sw tr al v 10` is the replacing form of the
//! allowed-VLAN list that black-holes a trunk.
//!
//! Rather than re-implement a device's parser (and get it subtly wrong), the
//! planner refuses abbreviated keywords in a `Lines` intent and shows the full
//! form. Lines computed from an `Intended` configuration are never abbreviated
//! — they come out of `hier::remediate`, which writes keywords in full.
//!
//! Only the leading keywords of a line are examined: everything from the first
//! value (`10`, `Gi0/1`, `WEB`) or free-text word (`description to core sw`)
//! onwards is left alone. A word that is not a prefix of any keyword Nocturne
//! knows is *unknown*, not abbreviated: the line is kept as typed, and the
//! caller is told so it can refuse to call the change low risk.

/// What a line's leading keywords turned out to be.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Every leading keyword was written out in full and in lower case.
    Full,
    /// A keyword was abbreviated or miscased; the message names the full form.
    Refuse(String),
    /// A word Nocturne does not know. It may be perfectly valid — a platform
    /// Nocturne has no vocabulary for, or a command added by a newer release —
    /// but its effect cannot be judged.
    Unknown(String),
}

fn miscased(token: &str, lower: &str) -> Verdict {
    Verdict::Refuse(format!(
        "`{token}` must be written in lower case (`{lower}`): Nocturne sends configuration exactly as it is written here and compares it with what the device prints back"
    ))
}

/// Examine the leading keywords of one configuration line.
pub fn inspect(line: &str) -> Verdict {
    let line = line.trim();
    let mut skip_next = false;
    // Keywords that may stand between a `NAMED` keyword and the name.
    let mut qualifiers: &[&str] = &[];
    for raw in line.split_whitespace() {
        let token = raw.trim_end_matches(',');
        if skip_next {
            // `class-map match-any NAME`, `service-policy output NAME`: the
            // name is the word after the qualifier, which is a keyword.
            let lower = token.to_ascii_lowercase();
            if qualifiers.contains(&lower.as_str()) {
                if token != lower {
                    return miscased(token, &lower);
                }
                qualifiers = &[];
                continue;
            }
            // The word after `username` (and friends) is a name somebody chose.
            skip_next = false;
            qualifiers = &[];
            continue;
        }
        if token.is_empty() {
            return Verdict::Full;
        }
        let lower = token.to_ascii_lowercase();
        if is_keyword(&lower) {
            if token != lower {
                return miscased(token, &lower);
            }
            if FREE_TEXT.contains(&lower.as_str()) || VALUES.contains(&lower.as_str()) {
                // The rest of the line is a value chosen by a human.
                return Verdict::Full;
            }
            skip_next = NAMED.contains(&lower.as_str());
            qualifiers = match lower.as_str() {
                "class-map" => &["match-any", "match-all"],
                "service-policy" => &["input", "output"],
                _ => &[],
            };
            continue;
        }
        if !token
            .chars()
            .all(|c| c.is_ascii_lowercase() || c == '-' || c == '_')
        {
            // A value: an interface name, an address, a number, a label.
            return Verdict::Full;
        }
        let candidates: Vec<&&str> = KEYWORDS
            .iter()
            .filter(|keyword| keyword.starts_with(&lower) && **keyword != lower)
            .take(4)
            .collect();
        return match candidates.as_slice() {
            [] => Verdict::Unknown(token.to_string()),
            [one] => Verdict::Refuse(format!(
                "`{token}` is an abbreviation of `{one}`; write the keyword out in full. \
                 A change is read by people and compared with the device afterwards, so Nocturne will not guess what an abbreviation meant."
            )),
            many => Verdict::Refuse(format!(
                "`{token}` is an abbreviation ({}); write the keyword out in full.",
                many.iter()
                    .map(|keyword| format!("`{keyword}`"))
                    .collect::<Vec<_>>()
                    .join(" or ")
            )),
        };
    }
    Verdict::Full
}

fn is_keyword(lower: &str) -> bool {
    KEYWORDS.binary_search(&lower).is_ok()
}

/// Keywords after which the next word is a name, not a keyword. A class or a
/// policy is named by whoever wrote it, in any case: `class voice`,
/// `service-policy input int` (after an optional qualifier, see `inspect`).
const NAMED: &[&str] = &[
    "username",
    "vrf",
    "route-map",
    "class",
    "class-map",
    "policy-map",
    "service-policy",
    "prefix-list",
    "keychain",
    "tacacs-server",
    "radius-server",
    "track",
];

/// Keywords after which the rest of the line is values: markings by number
/// or name (`match dscp ef af41`, `set cos 5`), whatever words they are.
const VALUES: &[&str] = &["dscp", "cos", "precedence"];

/// Keywords after which the rest of the line is free text.
const FREE_TEXT: &[&str] = &[
    "description",
    "name",
    "banner",
    "remark",
    "hostname",
    "alias",
    "contact",
    "location",
    "key-string",
    "comment",
];

/// Configuration keywords Nocturne knows, sorted (`binary_search`).
///
/// It does not have to be complete: a word that is not here and is not a
/// prefix of anything here is reported as unknown, which keeps the line but
/// stops the change from being called low risk.
const KEYWORDS: &[&str] = &[
    "aaa",
    "access",
    "access-class",
    "access-group",
    "access-list",
    "accounting",
    "activate",
    "add",
    "address",
    "address-family",
    "alias",
    "all",
    "allowed",
    "any",
    "area",
    "arp",
    "authentication",
    "authorization",
    "auto",
    "auto-cost",
    "aux",
    "average",
    "backup",
    "bandwidth",
    "banner",
    "bgp",
    "boot",
    "both",
    "bpduguard",
    "bridge-domain",
    "broadcast",
    "cdp",
    "channel-group",
    "channel-protocol",
    "cir",
    "class",
    "class-default",
    "class-map",
    "clock",
    "comment",
    "con",
    "conform-action",
    "connected",
    "console",
    "contact",
    "cos",
    "cost",
    "crypto",
    "dead-interval",
    "default",
    "default-information",
    "default-router",
    "defaultoriginate",
    "delay",
    "deny",
    "description",
    "dhcp",
    "dhcpv6",
    "disable",
    "domain",
    "domain-list",
    "domain-lookup",
    "domain-name",
    "dot1q",
    "drop",
    "dscp",
    "duplex",
    "dynamic",
    "edge",
    "enable",
    "encapsulation",
    "end",
    "eq",
    "errdisable",
    "exceed-action",
    "except",
    "exec",
    "exec-timeout",
    "exit",
    "export",
    "extended",
    "fair-queue",
    "filter",
    "first",
    "flowcontrol",
    "forward-protocol",
    "forwarding",
    "full",
    "gateway",
    "global",
    "gt",
    "guard",
    "half",
    "hello-interval",
    "helper-address",
    "history",
    "host",
    "hostname",
    "http",
    "icmp",
    "idle",
    "igmp",
    "import",
    "in",
    "input",
    "inside",
    "instance",
    "interface",
    "ip",
    "ipv6",
    "isis",
    "key",
    "key-string",
    "keychain",
    "lacp",
    "last",
    "lease",
    "level",
    "limit",
    "line",
    "link",
    "link-status",
    "lldp",
    "load-interval",
    "local",
    "location",
    "log",
    "logging",
    "login",
    "loopback",
    "lt",
    "mac",
    "mac-address",
    "match",
    "match-all",
    "match-any",
    "max",
    "maximum",
    "maximum-paths",
    "md5",
    "member",
    "mls",
    "mode",
    "monitor",
    "motd",
    "mst",
    "mtu",
    "multicast",
    "name",
    "name-server",
    "nameif",
    "native",
    "negotiate",
    "negotiation",
    "neighbor",
    "network",
    "next-hop",
    "no",
    "none",
    "nonegotiate",
    "nssa",
    "ntp",
    "ospf",
    "out",
    "output",
    "outside",
    "pagp",
    "passive",
    "passive-interface",
    "password",
    "peer",
    "percent",
    "permit",
    "pim",
    "poe",
    "point-to-point",
    "police",
    "policy",
    "policy-map",
    "port",
    "port-channel",
    "port-priority",
    "portfast",
    "power",
    "pppoe",
    "pre-shared-key",
    "precedence",
    "preempt",
    "prefix-list",
    "primary",
    "priority",
    "priority-queue",
    "private-vlan",
    "privilege",
    "protocol",
    "proxy-arp",
    "pvst",
    "qos",
    "queue",
    "queue-limit",
    "radius",
    "random-detect",
    "range",
    "rapid-pvst",
    "rate",
    "rate-limit",
    "receive",
    "redistribute",
    "redundancy",
    "remaining",
    "remark",
    "remote-as",
    "remove",
    "reset",
    "restrict",
    "root",
    "route",
    "route-map",
    "route-static",
    "router",
    "router-id",
    "routing",
    "rp-address",
    "runts",
    "scheduler",
    "secondary",
    "secret",
    "send",
    "sequence",
    "server",
    "service",
    "service-policy",
    "session",
    "set",
    "shape",
    "shutdown",
    "snmp-server",
    "source",
    "source-interface",
    "spanning-tree",
    "speed",
    "ssh",
    "standard",
    "standby",
    "static",
    "sticky",
    "storm-control",
    "stub",
    "summary-address",
    "suppress",
    "switch",
    "switchport",
    "sysname",
    "system",
    "tacacs",
    "tacacs-server",
    "tag",
    "tcp",
    "telnet",
    "timeout",
    "timers",
    "timezone",
    "track",
    "traffic",
    "transmit",
    "transport",
    "trap",
    "trap-source",
    "traps",
    "trunk",
    "trust",
    "trusted",
    "tunnel",
    "udld",
    "udp",
    "undo",
    "unicast",
    "untrusted",
    "uplinkfast",
    "username",
    "version",
    "violate-action",
    "vlan",
    "vlans",
    "voice",
    "vrf",
    "vrrp",
    "vrrpv3",
    "vtp",
    "vty",
    "wait",
    "write",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keyword_list_is_sorted_and_unique() {
        let mut sorted = KEYWORDS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted, KEYWORDS,
            "KEYWORDS must be sorted for binary_search"
        );
    }

    #[test]
    fn full_lines_pass() {
        for line in [
            "interface GigabitEthernet0/1",
            "switchport trunk allowed vlan add 10,20",
            "no shutdown",
            "ip address 10.0.0.2 255.255.255.0",
            "description to core sw",
            "name Voice",
            "banner motd ^C Keep out ^C",
            "spanning-tree portfast",
            "vlan 30",
            "ntp server 192.0.2.1",
            "ip access-list extended WEB",
            "permit tcp any any eq 80",
            "line vty 0 4",
            "exit",
            "",
        ] {
            assert_eq!(inspect(line), Verdict::Full, "{line}");
        }
    }

    #[test]
    fn abbreviations_are_refused_with_the_full_form() {
        let message = match inspect("int gi0/1") {
            Verdict::Refuse(message) => message,
            other => panic!("expected a refusal, got {other:?}"),
        };
        assert!(message.contains("`interface`"), "{message}");

        let message = match inspect("sw tr al v 10") {
            Verdict::Refuse(message) => message,
            other => panic!("expected a refusal, got {other:?}"),
        };
        assert!(message.contains("`switchport`"), "{message}");

        assert!(matches!(inspect("no shut"), Verdict::Refuse(_)));
        assert!(matches!(inspect("desc uplink"), Verdict::Refuse(_)));
        assert!(matches!(inspect("spanning-tree portf"), Verdict::Refuse(_)));
        // Ambiguous abbreviations name the candidates.
        match inspect("se something") {
            Verdict::Refuse(message) => assert!(message.contains(" or "), "{message}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn keywords_must_be_lower_case_so_that_text_can_be_compared() {
        match inspect("INTERFACE Gi0/1") {
            Verdict::Refuse(message) => assert!(message.contains("lower case"), "{message}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert!(matches!(
            inspect("  Switchport   Mode   trunk "),
            Verdict::Refuse(_)
        ));
        // A label in capitals is a value, not a keyword.
        assert_eq!(inspect("ip access-list extended WEB"), Verdict::Full);
    }

    #[test]
    fn a_class_or_policy_name_is_a_name_in_any_case() {
        // The word after `class-map` used to be skipped as the name, and for
        // `class-map match-any NAME` that word is `match-any`: the name was
        // then examined as a keyword — refused in capitals, unknown in lower
        // case, and refused outright where it looked like an abbreviation.
        for line in [
            "class-map match-any voice",
            "class-map match-all voice",
            "class-map match-any VOICE",
            "class-map voice",
            "policy-map voice",
            "class voice",
            "class VOICE",
            "class net",
            "class class-default",
            "service-policy output voice",
            "service-policy input int",
            "service-policy WAN-EDGE",
            "no class-map match-any int",
        ] {
            assert_eq!(inspect(line), Verdict::Full, "{line}");
        }
        // The qualifier is a keyword like any other.
        assert!(matches!(
            inspect("class-map MATCH-ANY voice"),
            Verdict::Refuse(message) if message.contains("lower case")
        ));
    }

    #[test]
    fn the_words_of_modular_qos_are_words_not_abbreviations_of_others() {
        for line in [
            "set cos 5",
            "match cos 3 4",
            "match dscp ef af41",
            "match ip dscp cs5",
            "set dscp ef",
            "match precedence 5",
            "priority percent 10",
            "bandwidth remaining percent 30",
            "shape average 50000000",
            "police cir 8000 conform-action transmit exceed-action drop",
            "conform-action transmit",
            "exceed-action drop",
            "violate-action drop",
            "random-detect",
            "fair-queue",
            "queue-limit 64 packets",
            "mls qos",
            "mls qos trust dscp",
            "priority-queue out",
            "rate-limit output 8000 1500 2000 conform-action transmit exceed-action drop",
        ] {
            assert_eq!(inspect(line), Verdict::Full, "{line}");
        }
        // Cut short, they are still refused.
        assert!(matches!(inspect("pol 8000"), Verdict::Refuse(_)));
        assert!(matches!(inspect("shape av 8000"), Verdict::Refuse(_)));
    }

    #[test]
    fn unknown_words_are_reported_as_unknown_not_guessed() {
        assert_eq!(
            inspect("frobnicate the-widget"),
            Verdict::Unknown("frobnicate".into())
        );
        assert_eq!(
            inspect("switchport frobnicate"),
            Verdict::Unknown("frobnicate".into())
        );
    }
}

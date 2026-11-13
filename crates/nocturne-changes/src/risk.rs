//! How dangerous a plan is, and why — in words the person approving it reads.
//!
//! `low` is a small category on purpose: it is what the assistant may be
//! allowed to apply by itself, so it is reserved for additive changes to
//! access ports, descriptions, VLAN names and the "tell me about yourself"
//! lines (NTP, logging, SNMP location). Anything that removes configuration,
//! anything Nocturne could not predict, anything on a device type that has
//! never been checked against real hardware, and anything that cannot be
//! undone automatically is more than that.

use std::collections::BTreeSet;

use nocturne_drivers::hier::Syntax;
use nocturne_drivers::profile::Maturity;

use crate::model::{ProtectedHit, Risk, RiskLevel};
use crate::protect;

/// Everything the risk level is worked out from.
pub struct Inputs<'a> {
    pub commands: &'a [String],
    pub syntax: Syntax,
    pub protected_hits: &'a [ProtectedHit],
    /// The rollback was computed and was shown to restore the configuration.
    pub rollback_ok: bool,
    pub maturity: Maturity,
    /// The platform has a dead-man switch.
    pub has_guard: bool,
    /// Words in the commands that Nocturne does not recognise.
    pub unknown_keywords: &'a [String],
    /// The result of the commands could be predicted (and so can be verified).
    pub prediction_supported: bool,
}

/// Commands that only ever add a leaf setting of the kinds a `low` risk
/// change is allowed to contain.
fn is_low_risk_line(line: &str) -> bool {
    const ALLOWED: &[&str] = &[
        "description ",
        "name ",
        "switchport mode access",
        "switchport access vlan ",
        "switchport voice vlan ",
        "spanning-tree portfast",
        "spanning-tree bpduguard ",
        "ntp server ",
        "ntp source ",
        "logging host ",
        "logging trap ",
        "logging buffered ",
        "logging source-interface ",
        "snmp-server location ",
        "snmp-server contact ",
        "clock timezone ",
        "ip domain-name ",
        "ip name-server ",
        "exit",
        "end",
    ];
    let line = line.trim().to_ascii_lowercase();
    // A section header for a port or a VLAN is fine; a routing process,
    // an access list or a VTY line is not.
    if line.starts_with("interface ") || line.starts_with("vlan ") {
        return !line.contains("range ");
    }
    ALLOWED
        .iter()
        .any(|allowed| line == allowed.trim_end() || line.starts_with(allowed))
}

/// Commands that take a whole section away.
fn removes_a_section(line: &str) -> bool {
    let line = line.trim().to_ascii_lowercase();
    if line.starts_with("default interface ") {
        return true;
    }
    let Some(target) = line
        .strip_prefix("no ")
        .or_else(|| line.strip_prefix("undo "))
    else {
        return false;
    };
    [
        "interface ",
        "vlan ",
        "router ",
        "line ",
        "ip access-list ",
        "ipv6 access-list ",
        "route-map ",
        "policy-map ",
        "class-map ",
        "vrf definition ",
        "ip dhcp pool ",
    ]
    .iter()
    .any(|section| target.starts_with(section))
}

pub fn assess(inputs: &Inputs<'_>) -> Risk {
    let mut reasons: Vec<String> = Vec::new();
    let located = protect::locate(inputs.commands, inputs.syntax);
    let sections: BTreeSet<&String> = located
        .iter()
        .flat_map(|item| item.sections.iter())
        .collect();
    let removals: Vec<&String> = inputs
        .commands
        .iter()
        .filter(|line| removes_a_section(line))
        .collect();
    let negations = inputs
        .commands
        .iter()
        .filter(|line| {
            let line = line.trim().to_ascii_lowercase();
            line.starts_with("no ") || line.starts_with("undo ") || line.starts_with("default ")
        })
        .count();

    let all_low = inputs.commands.iter().all(|line| is_low_risk_line(line))
        && negations == 0
        && inputs.unknown_keywords.is_empty();
    let mut level = if all_low {
        RiskLevel::Low
    } else {
        reasons.push(if negations > 0 {
            format!("{negations} of the commands remove configuration")
        } else {
            "the change is more than descriptions, VLAN names and access-port settings".into()
        });
        RiskLevel::Medium
    };

    // Protected resources decide the floor.
    if !inputs.protected_hits.is_empty() {
        let critical = inputs
            .protected_hits
            .iter()
            .any(|hit| protect::is_management_or_access(&hit.kind));
        level = level.max(if critical {
            RiskLevel::Critical
        } else {
            RiskLevel::High
        });
        let kinds: BTreeSet<&str> = inputs
            .protected_hits
            .iter()
            .map(|hit| hit.kind.as_str())
            .collect();
        reasons.insert(
            0,
            format!(
                "it touches protected resources ({})",
                kinds.into_iter().collect::<Vec<_>>().join(", ")
            ),
        );
    }

    if sections.len() > 20 {
        level = level.max(RiskLevel::Medium);
        reasons.push(format!(
            "it touches {} configuration sections",
            sections.len()
        ));
    }
    if !removals.is_empty() {
        level = level.max(RiskLevel::Medium);
        reasons.push(format!(
            "it removes whole sections ({})",
            removals
                .iter()
                .take(4)
                .map(|line| format!("`{}`", line.trim()))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !inputs.unknown_keywords.is_empty() {
        level = level.max(RiskLevel::Medium);
        reasons.push(format!(
            "it contains commands Nocturne does not know ({}), so their effect cannot be judged",
            inputs
                .unknown_keywords
                .iter()
                .take(4)
                .map(|word| format!("`{word}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !inputs.prediction_supported {
        level = level.max(RiskLevel::Medium);
        reasons.push(
            "the result of these commands cannot be predicted on this platform, so it cannot be verified afterwards"
                .into(),
        );
    }
    if !inputs.rollback_ok {
        level = level.raised();
        reasons.push("it cannot be undone automatically".into());
    }
    if inputs.maturity != Maturity::Verified {
        level = level.raised();
        reasons.push(format!(
            "the device type has not been verified against real hardware (`{}`)",
            inputs.maturity.as_str()
        ));
    }
    if !inputs.has_guard && level >= RiskLevel::High {
        level = level.raised();
        reasons.push(
            "this device type has no dead-man switch, so a change that cuts the session cannot undo itself"
                .into(),
        );
    }
    if reasons.is_empty() {
        reasons.push(
            "it only adds descriptions, VLAN names, access-port settings or management lines"
                .into(),
        );
    }
    Risk { level, reasons }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs<'a>(commands: &'a [String], hits: &'a [ProtectedHit]) -> Inputs<'a> {
        Inputs {
            commands,
            syntax: Syntax::Ios,
            protected_hits: hits,
            rollback_ok: true,
            maturity: Maturity::Verified,
            has_guard: true,
            unknown_keywords: &[],
            prediction_supported: true,
        }
    }

    fn lines(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| line.to_string()).collect()
    }

    #[test]
    fn additive_access_port_work_is_low() {
        let commands = lines(&[
            "interface GigabitEthernet0/2",
            "description desk 14",
            "switchport access vlan 20",
            "exit",
            "vlan 30",
            "name Voice",
            "exit",
            "ntp server 192.0.2.1",
        ]);
        let risk = assess(&inputs(&commands, &[]));
        assert_eq!(risk.level, RiskLevel::Low, "{:?}", risk.reasons);
    }

    #[test]
    fn removals_and_unknown_commands_are_never_low() {
        let commands = lines(&["no vlan 20"]);
        let risk = assess(&inputs(&commands, &[]));
        assert_eq!(risk.level, RiskLevel::Medium);
        assert!(risk.reasons.iter().any(|reason| reason.contains("remove")));

        let commands = lines(&["frobnicate the-widget"]);
        let unknown = vec!["frobnicate".to_string()];
        let risk = assess(&Inputs {
            unknown_keywords: &unknown,
            ..inputs(&commands, &[])
        });
        assert_eq!(risk.level, RiskLevel::Medium);
        assert!(
            risk.reasons
                .iter()
                .any(|reason| reason.contains("does not know"))
        );
    }

    #[test]
    fn protected_hits_decide_the_floor_and_the_extras_raise_it() {
        let commands = lines(&["interface Vlan1", "shutdown"]);
        let uplink = [ProtectedHit {
            kind: "uplink".into(),
            subject: "Gi0/1".into(),
            command: "shutdown".into(),
            why: String::new(),
        }];
        assert_eq!(assess(&inputs(&commands, &uplink)).level, RiskLevel::High);

        let management = [ProtectedHit {
            kind: "management_interface".into(),
            subject: "Vlan1".into(),
            command: "shutdown".into(),
            why: String::new(),
        }];
        assert_eq!(
            assess(&inputs(&commands, &management)).level,
            RiskLevel::Critical
        );

        // An uplink hit on an unverified profile with no guard: high + 1 + 1.
        let risk = assess(&Inputs {
            maturity: Maturity::Community,
            has_guard: false,
            ..inputs(&commands, &uplink)
        });
        assert_eq!(risk.level, RiskLevel::Critical);
        assert!(
            risk.reasons
                .iter()
                .any(|reason| reason.contains("dead-man switch"))
                && risk
                    .reasons
                    .iter()
                    .any(|reason| reason.contains("not been verified"))
        );
    }

    #[test]
    fn a_change_that_cannot_be_undone_is_a_level_worse() {
        let commands = lines(&["interface GigabitEthernet0/2", "description desk 14"]);
        let risk = assess(&Inputs {
            rollback_ok: false,
            ..inputs(&commands, &[])
        });
        assert_eq!(risk.level, RiskLevel::Medium);
        assert!(
            risk.reasons
                .iter()
                .any(|reason| reason.contains("cannot be undone"))
        );
    }

    #[test]
    fn very_wide_changes_are_at_least_medium() {
        let mut commands = Vec::new();
        for port in 1..=25 {
            commands.push(format!("interface GigabitEthernet0/{port}"));
            commands.push("description access port".to_string());
            commands.push("exit".to_string());
        }
        let risk = assess(&inputs(&commands, &[]));
        assert_eq!(risk.level, RiskLevel::Medium);
        assert!(
            risk.reasons
                .iter()
                .any(|reason| reason.contains("25 configuration sections"))
        );
    }
}

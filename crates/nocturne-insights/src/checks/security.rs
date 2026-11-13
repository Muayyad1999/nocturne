//! Plain reads of the stored configuration for the things that should not be
//! there.
//!
//! All of these understand IOS-style configurations (`Syntax::Ios`, which
//! covers IOS, IOS-XE, NX-OS, EOS and their relatives) and nothing else. A
//! device whose configuration is in another grammar is **not evaluated**
//! rather than reported as clean: a RouterOS device with telnet enabled is
//! not something this crate can see yet, and saying nothing is better than
//! saying "fine".

use std::net::IpAddr;

use nocturne_drivers::hier::Syntax;

use crate::checks::skip_rest;
use crate::model::{Finding, Severity, Suggestion};
use crate::norm;
use crate::report::CheckReport;
use crate::settings::Settings;
use crate::world::{Config, DeviceData, World};

const UNREADABLE: &str =
    "no stored configuration, or one in a grammar these checks do not read (IOS-style only)";

/// The devices whose configuration these checks can read at all.
fn readable<'a>(world: &'a World, report: &mut CheckReport) -> Vec<(&'a DeviceData, &'a Config)> {
    let mut readable = Vec::new();
    for data in world.targets() {
        match data.config.as_ref() {
            Some(config) if config.syntax == Syntax::Ios => readable.push((data, config)),
            _ => report.skip(data.id(), UNREADABLE),
        }
    }
    readable
}

/// Run `judge` for every device with a readable configuration.
fn for_each_config(
    world: &World,
    report: &mut CheckReport,
    judge: impl Fn(&DeviceData, &Config, &mut CheckReport),
) {
    for (data, config) in readable(world, report) {
        judge(data, config, report);
    }
    skip_rest(world, report, UNREADABLE);
}

// ------------------------------------------------------------------- telnet

/// A VTY line that accepts telnet.
///
/// Look-alike: a line with **no `transport input` line at all**. What the
/// platform does by default has changed between releases and differs between
/// platforms, so a configuration that does not say is not evaluated — rather
/// than reported either way.
pub fn telnet_enabled(world: &World, _settings: &Settings, report: &mut CheckReport) {
    for_each_config(world, report, |data, config, report| {
        let lines = norm::vty_lines(config);
        if lines.is_empty() {
            report.skip(data.id(), "the configuration has no `line vty` block");
            return;
        }
        let mut offending: Vec<(String, String)> = Vec::new();
        let mut any_explicit = false;
        for line in &lines {
            let Some(transport) = line.children.iter().find_map(|child| {
                child
                    .text
                    .strip_prefix("transport input ")
                    .map(str::trim)
                    .map(str::to_string)
            }) else {
                continue;
            };
            any_explicit = true;
            let allows_telnet = transport
                .split_whitespace()
                .any(|word| word == "telnet" || word == "all");
            if allows_telnet {
                offending.push((line.text.clone(), transport));
            }
        }
        if !any_explicit {
            report.skip(
                data.id(),
                "no `transport input` is configured on the VTY lines, and the platform default is \
                 not knowable from the configuration",
            );
            return;
        }
        report.ok(data.id());
        if offending.is_empty() {
            return;
        }
        let mut finding = Finding::new(
            "telnet_enabled",
            data.id(),
            "line vty",
            format!("{} accepts telnet for management", data.name()),
        )
        .detail(
            "Telnet carries the username, the password and the whole session in clear text. \
             Anyone on a shared segment, a mirrored port or a compromised host between the \
             operator and the device can read it and replay it.",
        );
        for (line, transport) in &offending {
            finding = finding.evidence(line.clone(), format!("transport input {transport}"));
        }
        report.found(
            finding.suggest(
                Suggestion::text(
                    "Make sure SSH works first — keys generated and a user that can log in — and \
                     then restrict the VTY lines to it. Doing it the other way round locks \
                     everybody out.",
                )
                .lines(
                    offending
                        .iter()
                        .flat_map(|(line, _)| [line.clone(), "transport input ssh".to_string()])
                        .collect::<Vec<_>>(),
                )
                .commands(vec!["show running-config | section line vty".to_string()]),
            ),
        );
    });
}

// --------------------------------------------------------------------- http

/// The unencrypted web interface is on.
pub fn http_server_enabled(world: &World, _settings: &Settings, report: &mut CheckReport) {
    for_each_config(world, report, |data, config, report| {
        let on = config.section("ip http server").is_some();
        let off = config.section("no ip http server").is_some();
        if !on && !off {
            report.skip(
                data.id(),
                "the configuration does not mention the HTTP server either way",
            );
            return;
        }
        report.ok(data.id());
        if !on {
            return;
        }
        report.found(
            Finding::new(
                "http_server_enabled",
                data.id(),
                "ip http server",
                format!("{} is running the unencrypted web interface", data.name()),
            )
            .detail(
                "`ip http server` is on by default on many images and is rarely used on purpose. \
                 It accepts credentials over plain HTTP and has a long history of its own \
                 vulnerabilities.",
            )
            .evidence("Configuration", "ip http server".to_string())
            .evidence(
                "HTTPS",
                if config.section("ip http secure-server").is_some() {
                    "`ip http secure-server` is also configured".to_string()
                } else {
                    "not configured".to_string()
                },
            )
            .suggest(
                Suggestion::text(
                    "Turn it off unless something uses it. If the web interface is needed, keep \
                     only `ip http secure-server` and restrict it with an access class.",
                )
                .lines(vec!["no ip http server".to_string()])
                .commands(vec!["show running-config | include ip http".to_string()]),
            ),
        );
    });
}

// --------------------------------------------------------------------- snmp

/// A community string straight out of the factory.
pub fn snmp_default_community(world: &World, _settings: &Settings, report: &mut CheckReport) {
    for_each_config(world, report, |data, config, report| {
        report.ok(data.id());
        for line in config.lines_starting("snmp-server community ") {
            let words: Vec<&str> = line.split_whitespace().collect();
            let Some(community) = words.get(2) else {
                continue;
            };
            if !matches!(
                community.to_ascii_lowercase().as_str(),
                "public" | "private"
            ) {
                continue;
            }
            let write = words
                .iter()
                .any(|word| word.eq_ignore_ascii_case("rw") || word.eq_ignore_ascii_case("write"));
            report.found(
                Finding::new(
                    "snmp_default_community",
                    data.id(),
                    format!("snmp community {community}"),
                    format!(
                        "{} has the default SNMP community `{community}`{}",
                        data.name(),
                        if write { " with write access" } else { "" }
                    ),
                )
                .severity(if write {
                    Severity::Critical
                } else {
                    Severity::High
                })
                .detail(if write {
                    "A writable community called `public` or `private` lets anyone who can reach \
                     the device change its configuration over UDP, without logging in. This is \
                     the single worst thing on a campus network and it is usually left over from \
                     a test."
                } else {
                    "A read community called `public` or `private` hands the whole device \
                     inventory, interface list, ARP table and routing table to anyone who can \
                     reach it."
                })
                .evidence("Configuration", line.to_string())
                .suggest(
                    Suggestion::text(
                        "Replace it with a community nobody can guess, or move to SNMPv3, and \
                         restrict it with an access list. Change the monitoring system at the \
                         same time or polling stops.",
                    )
                    .commands(vec![
                        "show running-config | include snmp-server community".to_string(),
                    ]),
                ),
            );
        }
    });
}

// ----------------------------------------------------------------- password

/// A reversible or offline-crackable enable secret.
pub fn weak_enable_password(world: &World, _settings: &Settings, report: &mut CheckReport) {
    for_each_config(world, report, |data, config, report| {
        let password = config.lines_starting("enable password ").next();
        let secret = config.lines_starting("enable secret ").next();
        if password.is_none() && secret.is_none() {
            report.skip(
                data.id(),
                "no enable password or secret in the configuration (the device may authenticate \
                 through AAA instead)",
            );
            return;
        }
        report.ok(data.id());
        // `enable secret 9 $9$…` (scrypt) and `8` (PBKDF2) are what should be
        // there; 0 is clear text, 5 is MD5, 7 is the reversible cipher.
        let kind = |line: &str| -> Option<(&'static str, Severity, String)> {
            let words: Vec<&str> = line.split_whitespace().collect();
            let is_secret = words.get(1) == Some(&"secret");
            let type_number = words.get(2).and_then(|word| word.parse::<u8>().ok());
            match (is_secret, type_number) {
                (_, Some(7)) => Some((
                    "a type 7 password, which is a published cipher and is reversed instantly",
                    Severity::High,
                    "7".into(),
                )),
                (_, Some(0)) | (_, None) => Some((
                    "stored in clear text in the configuration",
                    Severity::High,
                    "0 (clear text)".into(),
                )),
                (true, Some(5)) => Some((
                    "a type 5 (MD5) secret, which is cracked offline from any copy of the \
                     configuration",
                    Severity::Medium,
                    "5".into(),
                )),
                (true, Some(8)) | (true, Some(9)) => None,
                (false, Some(_)) => Some((
                    "an `enable password`, which is not a secret hash at all",
                    Severity::High,
                    "password".into(),
                )),
                (true, Some(_)) => None,
            }
        };
        for line in [password, secret].into_iter().flatten() {
            let Some((why, severity, which)) = kind(line) else {
                continue;
            };
            let command = line
                .split_whitespace()
                .take(2)
                .collect::<Vec<_>>()
                .join(" ");
            report.found(
                Finding::new(
                    "weak_enable_password",
                    data.id(),
                    command.clone(),
                    format!("{}'s `{command}` is {why}", data.name()),
                )
                .severity(severity)
                .detail(
                    "Anyone who gets a copy of the configuration — a backup, a TFTP server, a \
                     support case, a screenshot — gets privileged access to the device. The \
                     configuration is the least protected thing about a switch.",
                )
                // The value itself is never put in a finding.
                .evidence("Command", command)
                .evidence("Storage", which)
                .suggest(
                    Suggestion::text(
                        "Set `enable secret` with the strongest type the image supports (type 9 \
                         if it is offered, otherwise type 8) and remove `enable password`. Keep a \
                         console session open until you have tested the new secret.",
                    )
                    .commands(vec![
                        "show running-config | include enable (password|secret)".to_string(),
                    ]),
                ),
            );
        }
    });
}

// ------------------------------------------------------------------ vty acl

/// Management is reachable from anywhere.
///
/// Look-alike: access can be restricted somewhere this cannot see — a
/// control-plane policy, an upstream firewall, or a management VRF. The
/// finding says so, and it is low severity for that reason.
pub fn no_vty_acl(world: &World, _settings: &Settings, report: &mut CheckReport) {
    for_each_config(world, report, |data, config, report| {
        let lines = norm::vty_lines(config);
        if lines.is_empty() {
            report.skip(data.id(), "the configuration has no `line vty` block");
            return;
        }
        report.ok(data.id());
        let unrestricted: Vec<String> = lines
            .iter()
            .filter(|line| {
                !line
                    .children
                    .iter()
                    .any(|child| child.text.starts_with("access-class "))
            })
            .map(|line| line.text.clone())
            .collect();
        if unrestricted.is_empty() {
            return;
        }
        report.found(
            Finding::new(
                "no_vty_acl",
                data.id(),
                "line vty",
                format!(
                    "{} accepts management connections from anywhere",
                    data.name()
                ),
            )
            .detail(
                "No `access-class` on the VTY lines, so anything that can route to the device can \
                 try to log in — including a compromised host on a user VLAN. Access may be \
                 restricted elsewhere (a firewall, a control-plane policy); this only says the \
                 device itself does not do it.",
            )
            .evidence("Lines without an access class", unrestricted.join(", "))
            .suggest(
                Suggestion::text(
                    "Write an access list for the networks management comes from, test it from \
                     each of them, and only then apply it to the VTY lines — from a console \
                     session, because getting it wrong locks you out.",
                )
                .commands(vec!["show running-config | section line vty".to_string()]),
            ),
        );
    });
}

// --------------------------------------------------------------------- ntp

/// No time source.
pub fn ntp_not_configured(world: &World, _settings: &Settings, report: &mut CheckReport) {
    for_each_config(world, report, |data, config, report| {
        report.ok(data.id());
        let configured = ["ntp server ", "ntp peer ", "sntp server ", "ntp master"]
            .iter()
            .any(|prefix| config.has_line_starting(prefix));
        if configured {
            return;
        }
        report.found(
            Finding::new(
                "ntp_not_configured",
                data.id(),
                "ntp",
                format!("{} has no time source configured", data.name()),
            )
            .detail(
                "Every log line, every certificate check and every scheduled action on the device \
                 depends on its clock. Without NTP the clock drifts, and correlating this device's \
                 log with another device's stops working exactly when it is needed.",
            )
            .evidence("Configuration", "no `ntp server` or `ntp peer`".to_string())
            .suggest(
                Suggestion::text(
                    "Point it at the same two sources as the rest of the estate, and set the time \
                     zone while you are there.",
                )
                .commands(vec![
                    "show ntp status".to_string(),
                    "show clock".to_string(),
                ]),
            ),
        );
    });
}

// ------------------------------------------------------------------ syslog

/// The device keeps its log to itself.
pub fn no_syslog_host(world: &World, _settings: &Settings, report: &mut CheckReport) {
    for_each_config(world, report, |data, config, report| {
        report.ok(data.id());
        let destination = config.lines_starting("logging ").any(|line| {
            let rest = line.trim_start_matches("logging ").trim();
            let first = rest.split_whitespace().next().unwrap_or("");
            first == "host"
                || first == "server"
                || first.parse::<IpAddr>().is_ok()
                // `logging host 10.0.0.1` and `logging 10.0.0.1` cover IOS;
                // a hostname is accepted only after the explicit keyword, so
                // that `logging buffered 16384` is not read as a destination.
                || (first == "vrf" && rest.contains(" host "))
        });
        if destination {
            return;
        }
        report.found(
            Finding::new(
                "no_syslog_host",
                data.id(),
                "logging",
                format!("{} does not send its log anywhere", data.name()),
            )
            .detail(
                "Whatever the device says about a fault is held in a buffer that a reload empties. \
                 Port flaps, err-disable events and configuration changes are all things you only \
                 get to read once.",
            )
            .evidence("Configuration", "no `logging host`".to_string())
            .suggest(
                Suggestion::text(
                    "Send it to the collector the rest of the estate uses. Several of the checks \
                     here (flapping ports, err-disabled ports) can only see what the device \
                     reports.",
                )
                .commands(vec!["show logging".to_string()]),
            ),
        );
    });
}

#[cfg(test)]
#[path = "security_tests.rs"]
mod tests;

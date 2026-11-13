//! The configuration greps, case by case — including the platforms where
//! they refuse to answer.

use nocturne_drivers::hier::Syntax;

use crate::model::Severity;
use crate::testdata::*;
use crate::world::World;

fn configured(text: &str) -> World {
    world(vec![device("a", "SW1").config(text)])
}

const VTY_SSH: &str = "line vty 0 4\n transport input ssh\n access-class 10 in\n";

#[test]
fn telnet_enabled_table() {
    // Positive: a VTY line that accepts telnet.
    let found = run(
        "telnet_enabled",
        &configured("line vty 0 4\n transport input telnet ssh\n"),
    );
    assert_eq!(subjects(&found), ["a:line vty"]);
    assert_eq!(
        found.findings[0]
            .suggestion
            .as_ref()
            .unwrap()
            .config_lines
            .as_deref(),
        Some(
            [
                "line vty 0 4".to_string(),
                "transport input ssh".to_string()
            ]
            .as_slice()
        )
    );

    // Positive: `transport input all` includes telnet.
    assert_eq!(
        run(
            "telnet_enabled",
            &configured("line vty 0 4\n transport input all\n")
        )
        .findings
        .len(),
        1
    );

    // Negative: ssh only.
    assert!(
        run("telnet_enabled", &configured(VTY_SSH))
            .findings
            .is_empty()
    );

    // Look-alike / not enough data: no `transport input` at all. What the
    // platform does by default has changed between releases, so this is not
    // reported either way.
    let silent = run(
        "telnet_enabled",
        &configured("line vty 0 4\n login local\n"),
    );
    assert!(silent.findings.is_empty());
    assert_eq!(silent.skips().count(), 1);

    // Not evaluated: a configuration in a grammar these checks do not read. A
    // RouterOS device with telnet open is something this crate cannot see, and
    // saying nothing is better than saying "fine".
    let routeros = world(vec![
        device("a", "RB")
            .syntax(Syntax::Routeros)
            .config("/ip service set telnet disabled=no\n"),
    ]);
    let report = run("telnet_enabled", &routeros);
    assert!(report.findings.is_empty());
    assert_eq!(report.skips().count(), 1);

    // Not evaluated: nothing stored.
    assert_eq!(
        run("telnet_enabled", &world(vec![device("a", "SW1")]))
            .skips()
            .count(),
        1
    );
}

#[test]
fn http_server_enabled_table() {
    // Positive.
    let found = run("http_server_enabled", &configured("ip http server\n"));
    assert_eq!(subjects(&found), ["a:ip http server"]);
    assert_eq!(
        found.findings[0]
            .suggestion
            .as_ref()
            .unwrap()
            .config_lines
            .as_deref(),
        Some(["no ip http server".to_string()].as_slice())
    );

    // Negative: explicitly off.
    let off = run("http_server_enabled", &configured("no ip http server\n"));
    assert!(off.findings.is_empty());
    assert_eq!(off.evaluated().len(), 1);

    // Look-alike: HTTPS only. `ip http secure-server` is not the finding.
    assert!(
        run(
            "http_server_enabled",
            &configured("no ip http server\nip http secure-server\n")
        )
        .findings
        .is_empty()
    );

    // Not enough data: the configuration says nothing either way.
    let quiet = run("http_server_enabled", &configured("hostname SW1\n"));
    assert!(quiet.findings.is_empty());
    assert_eq!(quiet.skips().count(), 1);
}

#[test]
fn snmp_default_community_table() {
    // Positive: `public`, read-only.
    let found = run(
        "snmp_default_community",
        &configured("snmp-server community public RO\n"),
    );
    assert_eq!(found.findings.len(), 1);
    assert_eq!(found.findings[0].severity, Some(Severity::High));

    // Positive: writable, which is as bad as it gets.
    let write = run(
        "snmp_default_community",
        &configured("snmp-server community private RW\n"),
    );
    assert_eq!(write.findings[0].severity, Some(Severity::Critical));

    // Negative: a community nobody will guess.
    let good = run(
        "snmp_default_community",
        &configured("snmp-server community n0t-a-gu3ss RO 10\n"),
    );
    assert!(good.findings.is_empty());
    assert_eq!(good.evaluated().len(), 1);

    // Look-alike: a community that merely *contains* a default word.
    assert!(
        run(
            "snmp_default_community",
            &configured("snmp-server community public-transport-ro RO\n")
        )
        .findings
        .is_empty()
    );

    // Negative: no SNMP at all is decidable, not unknown.
    let none = run("snmp_default_community", &configured("hostname SW1\n"));
    assert!(none.findings.is_empty());
    assert_eq!(none.evaluated().len(), 1);
}

#[test]
fn weak_enable_password_table() {
    let lines = |text: &str| run("weak_enable_password", &configured(text));

    // Positive: `enable password` is not a hash at all.
    let password = lines("enable password 7 104B0718071B17\n");
    assert_eq!(password.findings.len(), 1);
    assert_eq!(password.findings[0].severity, Some(Severity::High));
    // The value itself never reaches a finding.
    assert!(
        password
            .findings
            .iter()
            .flat_map(|finding| finding.evidence.iter())
            .all(|item| !item.value.contains("104B0718071B17"))
    );
    assert!(!password.findings[0].title.contains("104B0718071B17"));

    // Positive: a type 5 secret, crackable offline.
    let md5 = lines("enable secret 5 $1$mERr$Kk1gkoHmFtbbvdOYmsbBq1\n");
    assert_eq!(md5.findings[0].severity, Some(Severity::Medium));

    // Negative: type 9 (scrypt).
    let modern = lines("enable secret 9 $9$abcdefghij$klmnopqrstuvwxyz\n");
    assert!(modern.findings.is_empty());
    assert_eq!(modern.evaluated().len(), 1);

    // Look-alike / not enough data: no enable line at all. The device may
    // authenticate through AAA, and calling that weak would be wrong.
    let aaa = lines("aaa authentication enable default group tacacs+ enable\n");
    assert!(aaa.findings.is_empty());
    assert_eq!(aaa.skips().count(), 1);
}

#[test]
fn no_vty_acl_table() {
    // Positive: no access class.
    let found = run(
        "no_vty_acl",
        &configured("line vty 0 4\n transport input ssh\n"),
    );
    assert_eq!(subjects(&found), ["a:line vty"]);

    // Negative: restricted.
    assert!(run("no_vty_acl", &configured(VTY_SSH)).findings.is_empty());

    // Look-alike: one of several VTY ranges is restricted and another is not
    // — that is still a way in, so it is reported, naming the line.
    let partial = run(
        "no_vty_acl",
        &configured("line vty 0 4\n access-class 10 in\nline vty 5 15\n transport input ssh\n"),
    );
    assert_eq!(partial.findings.len(), 1);
    assert!(
        partial.findings[0]
            .evidence
            .iter()
            .any(|item| item.value.contains("line vty 5 15"))
    );

    // Not enough data: no VTY block.
    let none = run("no_vty_acl", &configured("hostname SW1\n"));
    assert!(none.findings.is_empty());
    assert_eq!(none.skips().count(), 1);
}

#[test]
fn ntp_and_syslog_tables() {
    // Positive: neither is configured.
    let bare = configured("hostname SW1\nlogging buffered 16384\nlogging console warnings\n");
    assert_eq!(run("ntp_not_configured", &bare).findings.len(), 1);
    assert_eq!(run("no_syslog_host", &bare).findings.len(), 1);

    // Negative: both are.
    let looked_after = configured(
        "ntp server 192.0.2.1\nntp server 192.0.2.2\nlogging host 10.0.0.9\nlogging trap informational\n",
    );
    assert!(run("ntp_not_configured", &looked_after).findings.is_empty());
    assert!(run("no_syslog_host", &looked_after).findings.is_empty());

    // Look-alike 1: the old `logging <address>` syntax is a destination.
    assert!(
        run("no_syslog_host", &configured("logging 10.0.0.9\n"))
            .findings
            .is_empty()
    );

    // Look-alike 2: `logging buffered`/`logging console`/`logging trap` are
    // not destinations, and must not be read as one.
    assert_eq!(
        run(
            "no_syslog_host",
            &configured("logging buffered 64000\nlogging console\nlogging trap debugging\n")
        )
        .findings
        .len(),
        1
    );

    // Look-alike 3: `ntp master` (the device is the time source) counts.
    assert!(
        run("ntp_not_configured", &configured("ntp master 3\n"))
            .findings
            .is_empty()
    );

    // Not evaluated: nothing stored.
    for check in ["ntp_not_configured", "no_syslog_host"] {
        let report = run(check, &world(vec![device("a", "SW1")]));
        assert_eq!(report.skips().count(), 1, "{check}");
    }
}

/// Every configuration check refuses to answer for a grammar it does not
/// read, rather than reporting the device as clean.
#[test]
fn no_configuration_check_calls_an_unreadable_platform_healthy() {
    let routeros = world(vec![
        device("a", "RB")
            .syntax(Syntax::Routeros)
            .config("/system identity set name=RB\n"),
    ]);
    for check in [
        "telnet_enabled",
        "http_server_enabled",
        "snmp_default_community",
        "weak_enable_password",
        "no_vty_acl",
        "ntp_not_configured",
        "no_syslog_host",
    ] {
        let report = run(check, &routeros);
        assert!(report.findings.is_empty(), "{check}");
        assert_eq!(report.evaluated().len(), 0, "{check}");
        assert_eq!(report.skips().count(), 1, "{check}");
    }
}

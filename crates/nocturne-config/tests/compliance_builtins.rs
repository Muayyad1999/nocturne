//! Every rule of every built-in pack, against a configuration that must pass
//! it and one that must fail it.
//!
//! A hardening rule that is wrong about the platform is worse than no rule,
//! so each pack is held to both halves: the good configuration must not
//! produce a single finding, and the bad one must produce one for every rule.

mod support;

use serde_json::{Value, json};
use support::lab;

/// A Cisco IOS configuration that satisfies the whole hardening pack.
const IOS_GOOD: &str = "\
!
service password-encryption
hostname CORE-1
!
aaa new-model
aaa authentication login default group tacacs+ local
!
enable secret 9 $9$6Ks3$Hs7QMOHashOfTheEnableSecret
username ops privilege 15 secret 5 $1$Hb2W$HashOfTheUserPassword
!
no ip source-route
no ip http server
ip http secure-server
ip ssh version 2
ip domain-name lab.example
!
banner motd ^C
Authorised users only. Activity is logged.
^C
!
snmp-server community N0tTheDefault RO 7
snmp-server location Rack 3
!
logging host 10.0.0.9
ntp server 192.0.2.1 prefer
ntp server 192.0.2.2
!
interface GigabitEthernet0/1
 description uplink
 switchport mode trunk
!
line con 0
 exec-timeout 10 0
 logging synchronous
line aux 0
 transport input none
line vty 0 4
 exec-timeout 10 0
 transport input ssh
line vty 5 15
 exec-timeout 10 0
 transport input ssh
!
end
";

/// The same switch as it comes out of the box: every rule of the pack fails.
const IOS_BAD: &str = "\
!
hostname SWITCH
!
enable password cisco
username admin password 0 letmein
!
ip http server
!
snmp-server community public RO
snmp-server community private RW
!
interface GigabitEthernet0/1
 switchport mode access
!
line con 0
 exec-timeout 0 0
 password cisco
 login
line aux 0
 transport input all
line vty 0 4
 exec-timeout 0 0
 transport input telnet
 password cisco
 login
!
end
";

/// A RouterOS export that satisfies the whole hardening pack.
const ROS_GOOD: &str = "\
# nov/13/2026 12:00:00 by RouterOS 7.14
/ip service
set telnet disabled=yes
set ftp disabled=yes
set www disabled=yes
set api disabled=yes
/ip ssh
set strong-crypto=yes
/ip dns
set allow-remote-requests=no servers=192.0.2.53
/ip neighbor discovery-settings
set discover-interface-list=LAN
/tool mac-server
set allowed-interface-list=none
/tool mac-server mac-winbox
set allowed-interface-list=none
/tool bandwidth-server
set enabled=no
/snmp community
set [ find default=yes ] name=monitoring-only
";

/// A router left as it shipped, with a couple of things made worse.
const ROS_BAD: &str = "\
# nov/13/2026 12:00:00 by RouterOS 7.14
/ip service
set api-ssl disabled=yes
/ip dns
set allow-remote-requests=yes servers=192.0.2.53
/ip neighbor discovery-settings
set discover-interface-list=all
/snmp community
set [ find default=yes ] name=public
";

/// The verdict of every rule of a policy against one configuration.
async fn verdicts(policy_id: &str, text: &str, syntax: &str) -> Vec<(String, String, String)> {
    let lab = lab(&["SW1"]).await;
    let policy = lab
        .app
        .rpc("compliance.policies.get", json!({"policy_id": policy_id}))
        .await
        .expect("the built-in policy");
    let rules = policy["rules"].as_array().expect("rules").clone();
    assert!(!rules.is_empty());
    let mut out = Vec::new();
    for rule in rules {
        let result = lab
            .app
            .rpc(
                "compliance.test",
                json!({"rule": rule, "text": text, "syntax": syntax}),
            )
            .await
            .unwrap_or_else(|err| panic!("rule `{}` runs: {err}", rule["id"]));
        out.push((
            rule["id"].as_str().unwrap().to_string(),
            result["finding"]["status"].as_str().unwrap().to_string(),
            result["finding"]["detail"].as_str().unwrap().to_string(),
        ));
    }
    out
}

#[track_caller]
fn assert_all(verdicts: &[(String, String, String)], expected: &str) {
    let wrong: Vec<&(String, String, String)> = verdicts
        .iter()
        .filter(|(_, status, _)| status != expected)
        .collect();
    assert!(
        wrong.is_empty(),
        "these rules did not come out `{expected}`:\n{}",
        wrong
            .iter()
            .map(|(id, status, detail)| format!("  {id}: {status} — {detail}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[tokio::test]
async fn the_ios_pack_passes_a_hardened_configuration() {
    let verdicts = verdicts("builtin.ios.hardening", IOS_GOOD, "ios").await;
    assert_eq!(verdicts.len(), 16, "the pack's size is deliberate");
    assert_all(&verdicts, "pass");
}

#[tokio::test]
async fn the_ios_pack_fails_every_rule_on_a_device_as_it_shipped() {
    let verdicts = verdicts("builtin.ios.hardening", IOS_BAD, "ios").await;
    assert_all(&verdicts, "fail");
}

#[tokio::test]
async fn the_routeros_pack_passes_a_hardened_export() {
    let verdicts = verdicts("builtin.routeros.hardening", ROS_GOOD, "routeros").await;
    assert_eq!(verdicts.len(), 11, "the pack's size is deliberate");
    assert_all(&verdicts, "pass");
}

#[tokio::test]
async fn the_routeros_pack_fails_every_rule_on_an_export_left_at_its_defaults() {
    let verdicts = verdicts("builtin.routeros.hardening", ROS_BAD, "routeros").await;
    assert_all(&verdicts, "fail");
}

/// The traps each pack's patterns are written to avoid.
#[tokio::test]
async fn the_packs_do_not_mistake_a_near_miss_for_the_real_thing() {
    let lab = lab(&["SW1"]).await;
    let ask = async |policy_id: &str, rule_id: &str, text: &str, syntax: &str| -> Value {
        let policy = lab
            .app
            .rpc("compliance.policies.get", json!({"policy_id": policy_id}))
            .await
            .expect("the policy");
        let rule = policy["rules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|rule| rule["id"] == rule_id)
            .unwrap_or_else(|| panic!("rule `{rule_id}`"))
            .clone();
        lab.app
            .rpc(
                "compliance.test",
                json!({"rule": rule, "text": text, "syntax": syntax}),
            )
            .await
            .expect("the rule runs")["finding"]["status"]
            .clone()
    };

    // Disabling `api-ssl` is not disabling `api`, and `www-ssl` is not `www`.
    let ros = "/ip service\nset api-ssl disabled=yes\nset www-ssl disabled=yes\n";
    assert_eq!(
        ask(
            "builtin.routeros.hardening",
            "ros.api-disabled",
            ros,
            "routeros"
        )
        .await,
        "fail"
    );
    assert_eq!(
        ask(
            "builtin.routeros.hardening",
            "ros.www-disabled",
            ros,
            "routeros"
        )
        .await,
        "fail"
    );
    // Whichever way the export spells the selector, a disabled service passes.
    let ros = "/ip service\nset [ find name=telnet ] disabled=yes\n";
    assert_eq!(
        ask(
            "builtin.routeros.hardening",
            "ros.telnet-disabled",
            ros,
            "routeros"
        )
        .await,
        "pass"
    );

    // `logging buffered 16384` and `logging console` are not a syslog server.
    let ios = "logging buffered 16384\nlogging console warnings\nlogging trap debugging\n";
    assert_eq!(
        ask("builtin.ios.hardening", "ios.logging-host", ios, "ios").await,
        "fail"
    );
    for good in [
        "logging host 10.0.0.9\n",
        "logging 10.0.0.9\n",
        "logging host 2001:db8::9\n",
        "logging host syslog.example.com transport tcp port 1514\n",
    ] {
        assert_eq!(
            ask("builtin.ios.hardening", "ios.logging-host", good, "ios").await,
            "pass",
            "{good}"
        );
    }

    // A community whose name merely starts with `public` is still a finding,
    // but `publicity` is not the default community.
    assert_eq!(
        ask(
            "builtin.ios.hardening",
            "ios.snmp-default-community",
            "snmp-server community publicity RO\n",
            "ios"
        )
        .await,
        "pass"
    );
    assert_eq!(
        ask(
            "builtin.ios.hardening",
            "ios.snmp-default-community",
            "snmp-server community PUBLIC RW\n",
            "ios"
        )
        .await,
        "fail",
        "the case of a community name does not make it safe"
    );

    // `no ip http server` must not be read as `ip http server`.
    assert_eq!(
        ask(
            "builtin.ios.hardening",
            "ios.no-http-server",
            "no ip http server\nip http secure-server\n",
            "ios"
        )
        .await,
        "pass"
    );

    // A VTY block without an `exec-timeout` line is at the ten-minute default
    // and is fine; only an explicit `0 0` is a finding.
    assert_eq!(
        ask(
            "builtin.ios.hardening",
            "ios.vty-exec-timeout",
            "line vty 0 4\n transport input ssh\n",
            "ios"
        )
        .await,
        "pass"
    );

    // A device with no AUX port is not judged on it.
    assert_eq!(
        ask(
            "builtin.ios.hardening",
            "ios.aux-transport-none",
            "line vty 0 4\n transport input ssh\n",
            "ios"
        )
        .await,
        "not_applicable"
    );

    // `username … secret` passes where `username … password` fails, however
    // many options stand between the name and the keyword.
    for (text, expected) in [
        ("username a privilege 15 secret 9 $9$x\n", "pass"),
        ("username a secret 0 Cleartext1\n", "pass"),
        ("username a privilege 15 password 7 0822455D\n", "fail"),
        ("username a view SHELL password 0 letmein\n", "fail"),
    ] {
        assert_eq!(
            ask("builtin.ios.hardening", "ios.username-secret", text, "ios").await,
            expected,
            "{text}"
        );
    }
}

#[tokio::test]
async fn a_pack_only_judges_the_devices_it_is_about() {
    let lab = lab(&["SW1"]).await;
    // The fake switch is an IOS device (`os: IOS`), so the IOS pack covers it
    // and the RouterOS pack does not.
    lab.app
        .store()
        .config_insert(&lab.id("SW1"), "running", IOS_GOOD, "manual", "")
        .expect("a stored configuration");

    let run = lab
        .app
        .rpc("compliance.run", json!({}))
        .await
        .expect("the run");
    assert_eq!(run["summary"]["policies"], 1, "only the IOS pack applies");
    assert_eq!(run["summary"]["rules"], 16);
    assert_eq!(run["summary"]["fail"], 0);
    assert_eq!(run["summary"]["pass"], 16);
    assert_eq!(run["summary"]["score_pct"], 100.0);

    let results = lab
        .app
        .rpc("compliance.results", json!({}))
        .await
        .expect("the findings");
    assert!(
        results["findings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|finding| finding["policy_id"] == "builtin.ios.hardening")
    );
    // Two rules of the pack are critical: the VTY transport and the default
    // SNMP community. Both passed, so neither shows up as a failure.
    assert_eq!(
        results["summary"]["by_severity"]["critical"],
        json!({"pass": 2, "fail": 0})
    );
    assert_eq!(
        results["summary"]["by_severity"]["info"],
        json!({"pass": 0, "fail": 0}),
        "every severity is in the summary, whether it occurs or not"
    );
}

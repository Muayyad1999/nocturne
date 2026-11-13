//! Every `Check` variant against real-shaped configurations, through the
//! `compliance.test` method the rule editor uses.

mod support;

use serde_json::{Value, json};
use support::lab;

const IOS: &str = "\
!
version 15.2
service password-encryption
hostname SW1
!
aaa new-model
!
no ip http server
no ip source-route
!
interface GigabitEthernet0/1
 description uplink
 switchport mode trunk
 switchport trunk allowed vlan 10,20
!
interface GigabitEthernet0/2
 description desk 12
 switchport mode access
 switchport access vlan 10
 spanning-tree portfast
!
interface GigabitEthernet0/3
 description desk 13
 switchport    mode    access
 switchport access vlan 10
!
interface Vlan1
 ip address 10.0.0.2 255.255.255.0
!
line con 0
 exec-timeout 0 0
line vty 0 4
 transport input ssh
line vty 5 15
 transport input ssh telnet
!
ntp server 192.0.2.1
end
";

/// Ask one rule of one configuration.
async fn check(check: Value) -> Value {
    let lab = lab(&["SW1"]).await;
    let result = lab
        .app
        .rpc(
            "compliance.test",
            json!({
                "rule": {"id": "r1", "title": "A rule", "severity": "medium", "check": check},
                "text": IOS,
            }),
        )
        .await
        .expect("the rule runs");
    result["finding"].clone()
}

#[track_caller]
fn status(finding: &Value) -> &str {
    finding["status"].as_str().expect("a status")
}

#[track_caller]
fn evidence(finding: &Value) -> Vec<(u64, String)> {
    finding["evidence"]
        .as_array()
        .expect("evidence")
        .iter()
        .map(|line| {
            (
                line["line_no"].as_u64().unwrap(),
                line["line"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn present_is_an_exact_global_line_and_says_where_it_is() {
    let found = check(json!({"type": "present", "line": "aaa new-model"})).await;
    assert_eq!(status(&found), "pass");
    assert_eq!(evidence(&found), vec![(6, "aaa new-model".to_string())]);

    // Spacing in the rule does not matter.
    let found = check(json!({"type": "present", "line": "  aaa   new-model  "})).await;
    assert_eq!(status(&found), "pass");

    // A line inside a section is not a global line.
    let found = check(json!({"type": "present", "line": "transport input ssh"})).await;
    assert_eq!(status(&found), "fail");
    assert_eq!(found["missing"], json!(["transport input ssh"]));

    // Nor is a line that only starts the same way.
    let found = check(json!({"type": "present", "line": "ntp server"})).await;
    assert_eq!(status(&found), "fail");
    let found = check(json!({"type": "present", "line": "ip ssh version 2"})).await;
    assert_eq!(status(&found), "fail");
    assert!(found["detail"].as_str().unwrap().contains("not in the"));
}

#[tokio::test]
async fn absent_fails_when_the_line_is_there_and_quotes_it() {
    let found = check(json!({"type": "absent", "line": "ip http server"})).await;
    assert_eq!(
        status(&found),
        "pass",
        "`no ip http server` is a different line"
    );

    let found = check(json!({"type": "absent", "line": "no ip http server"})).await;
    assert_eq!(status(&found), "fail");
    assert_eq!(evidence(&found), vec![(8, "no ip http server".to_string())]);
}

#[tokio::test]
async fn matches_anchors_at_the_start_of_the_line_as_the_device_prints_it() {
    // A global line is offered to the pattern without indentation…
    let found = check(json!({"type": "matches", "pattern": "^ntp server \\S+"})).await;
    assert_eq!(status(&found), "pass");
    assert_eq!(
        evidence(&found),
        vec![(37, "ntp server 192.0.2.1".to_string())]
    );

    // …and a line inside a section with one space per level, so `^` tells them apart.
    let found = check(json!({"type": "matches", "pattern": "^description uplink"})).await;
    assert_eq!(status(&found), "fail");
    let found = check(json!({"type": "matches", "pattern": "^ description uplink"})).await;
    assert_eq!(status(&found), "pass");

    // `$` is the end of the line, not the end of the configuration.
    let found = check(json!({"type": "matches", "pattern": "^hostname SW1$"})).await;
    assert_eq!(status(&found), "pass");
    let found = check(json!({"type": "matches", "pattern": "^hostname$"})).await;
    assert_eq!(status(&found), "fail");

    // Whitespace is normalised before matching.
    let found = check(json!({"type": "matches", "pattern": "^ switchport mode access$"})).await;
    assert_eq!(
        status(&found),
        "pass",
        "the third port is written with wide spacing"
    );
}

#[tokio::test]
async fn not_matches_reports_every_offending_line() {
    let found = check(json!({"type": "not_matches", "pattern": "telnet"})).await;
    assert_eq!(status(&found), "fail");
    assert_eq!(
        evidence(&found),
        vec![(35, "transport input ssh telnet".to_string())]
    );
    assert!(found["detail"].as_str().unwrap().contains("1 line matches"));

    let found = check(json!({"type": "not_matches", "pattern": "^ switchport mode access$"})).await;
    assert_eq!(status(&found), "fail");
    assert_eq!(evidence(&found).len(), 2);
    assert!(found["detail"].as_str().unwrap().contains("2 lines match"));

    let found = check(json!({"type": "not_matches", "pattern": "^ip http server$"})).await;
    assert_eq!(status(&found), "pass");
}

#[tokio::test]
async fn an_invalid_pattern_is_refused_with_the_error() {
    let lab = lab(&["SW1"]).await;
    let err = lab
        .app
        .rpc(
            "compliance.test",
            json!({
                "rule": {"id": "r1", "title": "Broken", "check": {"type": "matches", "pattern": "("}},
                "text": IOS,
            }),
        )
        .await
        .expect_err("a broken pattern is refused");
    assert_eq!(err.code, "invalid");
    let problems = err.details.expect("the problems are listed");
    assert_eq!(problems["problems"][0]["field"], "check.pattern");
    assert!(
        problems["problems"][0]["message"]
            .as_str()
            .unwrap()
            .contains("regular expression"),
        "{problems}"
    );
}

#[tokio::test]
async fn section_with_each_judges_every_matching_section() {
    // Every VTY block must permit SSH only: one of the two does not.
    let found = check(json!({
        "type": "section",
        "parent": "^line vty",
        "each": true,
        "must": ["^transport input ssh$"],
    }))
    .await;
    assert_eq!(status(&found), "fail");
    assert_eq!(found["sections"], json!(["line vty 5 15"]));
    assert_eq!(found["missing"], json!(["^transport input ssh$"]));
    assert_eq!(evidence(&found), vec![(34, "line vty 5 15".to_string())]);
    assert!(
        found["detail"]
            .as_str()
            .unwrap()
            .contains("1 of 2 sections matching `^line vty` does not satisfy"),
        "{found}"
    );

    // Without `each`, one section that satisfies it is enough.
    let found = check(json!({
        "type": "section",
        "parent": "^line vty",
        "must": ["^transport input ssh$"],
    }))
    .await;
    assert_eq!(status(&found), "pass");
    assert_eq!(evidence(&found), vec![(32, "line vty 0 4".to_string())]);

    // A literal entry matches a line written with other spacing.
    let found = check(json!({
        "type": "section",
        "parent": "^interface GigabitEthernet0/3$",
        "each": true,
        "must": ["switchport mode access"],
    }))
    .await;
    assert_eq!(status(&found), "pass");
}

#[tokio::test]
async fn section_must_not_finds_the_line_that_should_not_be_there() {
    let found = check(json!({
        "type": "section",
        "parent": "^line (vty|con)",
        "each": true,
        "must_not": ["^exec-timeout 0 0$"],
    }))
    .await;
    assert_eq!(status(&found), "fail");
    assert_eq!(found["sections"], json!(["line con 0"]));
    assert_eq!(
        evidence(&found),
        vec![
            (30, "line con 0".to_string()),
            (31, "exec-timeout 0 0".to_string()),
        ]
    );
}

#[tokio::test]
async fn a_section_rule_about_nothing_is_not_applicable() {
    let found = check(json!({
        "type": "section",
        "parent": "^router bgp",
        "each": true,
        "must": ["^bgp log-neighbor-changes$"],
    }))
    .await;
    assert_eq!(status(&found), "not_applicable");
    assert!(
        found["detail"]
            .as_str()
            .unwrap()
            .contains("no section matching"),
        "{found}"
    );
    assert_eq!(found["evidence"], json!([]));
}

#[tokio::test]
async fn section_when_only_judges_the_sections_that_qualify() {
    // Access ports must have PortFast; the trunk and the SVI are not asked.
    let found = check(json!({
        "type": "section_when",
        "parent": "^interface ",
        "when": "^switchport mode access$",
        "must": ["^spanning-tree portfast$"],
    }))
    .await;
    assert_eq!(status(&found), "fail");
    assert_eq!(found["sections"], json!(["interface GigabitEthernet0/3"]));
    assert!(
        found["detail"]
            .as_str()
            .unwrap()
            .contains("1 of 2 sections"),
        "{found}"
    );

    // Nothing qualifies: the rule has nothing to say.
    let found = check(json!({
        "type": "section_when",
        "parent": "^interface ",
        "when": "^switchport mode dot1q-tunnel$",
        "must": ["^spanning-tree bpduguard enable$"],
    }))
    .await;
    assert_eq!(status(&found), "not_applicable");

    // Every qualifying section satisfies it.
    let found = check(json!({
        "type": "section_when",
        "parent": "^interface ",
        "when": "^switchport mode access$",
        "must": ["^switchport access vlan \\d+$"],
    }))
    .await;
    assert_eq!(status(&found), "pass");
    assert_eq!(evidence(&found).len(), 2);
}

#[tokio::test]
async fn a_snippet_passes_when_every_line_of_it_is_there_in_its_block() {
    let found = check(json!({
        "type": "snippet",
        "config": "service password-encryption\ninterface GigabitEthernet0/1\n switchport mode trunk\n",
    }))
    .await;
    assert_eq!(status(&found), "pass");

    let found = check(json!({
        "type": "snippet",
        "config": "interface GigabitEthernet0/1\n switchport mode trunk\n storm-control broadcast level 5.00\nlogging host 10.0.0.9\n",
    }))
    .await;
    assert_eq!(status(&found), "fail");
    assert_eq!(
        found["missing"],
        json!([
            "interface GigabitEthernet0/1",
            " storm-control broadcast level 5.00",
            "logging host 10.0.0.9"
        ]),
        "a block that exists is reported with only the lines it lacks"
    );
    assert!(found["detail"].as_str().unwrap().contains("3 lines"));
}

#[tokio::test]
async fn a_device_without_a_backup_is_not_evaluated_never_passed() {
    let lab = lab(&["SW1"]).await;
    let run = lab
        .app
        .rpc("compliance.run", json!({}))
        .await
        .expect("the run");
    let summary = &run["summary"];
    assert_eq!(summary["pass"], 0);
    assert_eq!(summary["fail"], 0);
    assert!(summary["not_evaluated"].as_u64().unwrap() > 10);
    assert_eq!(summary["devices_without_backup"], 1);
    assert!(summary["score_pct"].is_null(), "nothing was judged");

    let results = lab
        .app
        .rpc("compliance.results", json!({"status": "not_evaluated"}))
        .await
        .expect("the findings");
    let finding = &results["findings"][0];
    assert_eq!(finding["status"], "not_evaluated");
    assert!(finding["version_id"].is_null());
    assert!(
        finding["detail"]
            .as_str()
            .unwrap()
            .contains("no stored running configuration"),
        "{finding}"
    );
}

#[tokio::test]
async fn compliance_test_needs_something_to_check_against() {
    let lab = lab(&["SW1"]).await;
    let rule = json!({"id": "r1", "title": "t", "check": {"type": "present", "line": "x"}});

    let err = lab
        .app
        .rpc("compliance.test", json!({"rule": rule}))
        .await
        .expect_err("nothing to check");
    assert_eq!(err.code, "invalid");
    assert!(err.message.contains("`text`"), "{err}");

    let err = lab
        .app
        .rpc("compliance.test", json!({"text": IOS}))
        .await
        .expect_err("no rule");
    assert_eq!(err.code, "invalid");

    // Against a device that was never backed up.
    let err = lab
        .app
        .rpc(
            "compliance.test",
            json!({"rule": rule, "device_id": lab.id("SW1")}),
        )
        .await
        .expect_err("there is no configuration");
    assert_eq!(err.code, "not_found");
    assert!(err.message.contains("back the device up"), "{err}");
}

#[tokio::test]
async fn a_rule_can_be_tested_against_a_routeros_export() {
    let lab = lab(&["SW1"]).await;
    let export = "\
/ip service
set telnet disabled=yes
set www disabled=yes
/ip ssh
set strong-crypto=yes
";
    for (pattern, expected) in [
        (
            "^/ip service set (?:.*[ =])?telnet[ =].*\\bdisabled=yes\\b",
            "pass",
        ),
        (
            "^/ip service set (?:.*[ =])?ftp[ =].*\\bdisabled=yes\\b",
            "fail",
        ),
        ("^/ip ssh set .*\\bstrong-crypto=yes\\b", "pass"),
    ] {
        let result = lab
            .app
            .rpc(
                "compliance.test",
                json!({
                    "rule": {"id": "r", "title": "t", "check": {"type": "matches", "pattern": pattern}},
                    "text": export,
                    "syntax": "routeros",
                }),
            )
            .await
            .expect("the rule runs");
        assert_eq!(result["syntax"], "routeros");
        assert_eq!(
            result["finding"]["status"], expected,
            "`{pattern}` against the export"
        );
    }

    // A menu line is not a statement by itself, so it cannot match on its own.
    let result = lab
        .app
        .rpc(
            "compliance.test",
            json!({
                "rule": {"id": "r", "title": "t", "check": {"type": "present", "line": "/ip service"}},
                "text": export,
                "syntax": "routeros",
            }),
        )
        .await
        .expect("the rule runs");
    assert_eq!(result["finding"]["status"], "fail");
}

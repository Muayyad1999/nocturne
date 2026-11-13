//! Policies as objects people edit, runs as records that are kept, and the
//! hand-over of a fix to the change pipeline.

mod support;

use serde_json::{Value, json};
use support::{audits, lab, lab_with_changes};

const WEAK: &str = "\
hostname SW1
snmp-server community public RO
line vty 0 4
 transport input telnet
line vty 5 15
 transport input ssh telnet
";

fn policy(id: &str) -> Value {
    json!({
        "id": id,
        "name": "Site standard",
        "description": "What every switch here has",
        "enabled": true,
        "scope": {"os": ["ios"]},
        "rules": [{
            "id": "site.ntp",
            "title": "The site NTP server is configured",
            "severity": "medium",
            "rationale": "Logs that cannot be lined up are not evidence.",
            "check": {"type": "present", "line": "ntp server 192.0.2.1"},
            "remediation": ["ntp server 192.0.2.1"],
        }],
    })
}

#[tokio::test]
async fn policies_can_be_written_read_copied_and_deleted() {
    let lab = lab(&["SW1"]).await;
    let built_in = lab
        .app
        .rpc("compliance.policies.list", json!({}))
        .await
        .expect("the list");
    assert_eq!(built_in.as_array().unwrap().len(), 2);
    assert!(
        built_in
            .as_array()
            .unwrap()
            .iter()
            .all(|policy| policy["builtin"] == true && policy["enabled"] == true)
    );

    // A policy with no id is created with one.
    let mut incoming = policy("");
    incoming["id"] = json!("");
    let saved = lab
        .app
        .rpc("compliance.policies.save", json!({"policy": incoming}))
        .await
        .expect("the policy is saved");
    let id = saved["id"].as_str().unwrap().to_string();
    assert!(!id.is_empty());
    assert_eq!(saved["builtin"], false);
    assert!(saved["updated_at"].as_i64().unwrap() > 0);
    assert_eq!(
        audits(&lab.app, "compliance.policies.save")[0].message,
        "Created the compliance policy `Site standard` (1 rule)"
    );

    // Saving it again changes it in place.
    let mut changed = saved.clone();
    changed["name"] = json!("Site standard (2026)");
    let saved = lab
        .app
        .rpc("compliance.policies.save", json!({"policy": changed}))
        .await
        .expect("the policy is saved");
    assert_eq!(saved["name"], "Site standard (2026)");
    assert_eq!(
        lab.app
            .rpc("compliance.policies.list", json!({}))
            .await
            .expect("the list")
            .as_array()
            .unwrap()
            .len(),
        3
    );

    let copy = lab
        .app
        .rpc("compliance.policies.duplicate", json!({"policy_id": id}))
        .await
        .expect("the copy");
    assert_eq!(copy["name"], "Site standard (2026) (copy)");
    assert_ne!(copy["id"], saved["id"]);
    assert_eq!(copy["rules"], saved["rules"]);

    lab.app
        .rpc("compliance.policies.delete", json!({"policy_id": id}))
        .await
        .expect("the policy is deleted");
    let err = lab
        .app
        .rpc("compliance.policies.get", json!({"policy_id": id}))
        .await
        .expect_err("it is gone");
    assert_eq!(err.code, "not_found");
    assert_eq!(audits(&lab.app, "compliance.policies.delete").len(), 1);
}

#[tokio::test]
async fn a_built_in_policy_can_be_switched_off_but_not_edited() {
    let lab = lab(&["SW1"]).await;
    let mut built_in = lab
        .app
        .rpc(
            "compliance.policies.get",
            json!({"policy_id": "builtin.ios.hardening"}),
        )
        .await
        .expect("the built-in policy");

    // Changing a rule is refused, with what to do instead.
    let mut edited = built_in.clone();
    edited["rules"][0]["severity"] = json!("info");
    let err = lab
        .app
        .rpc("compliance.policies.save", json!({"policy": edited}))
        .await
        .expect_err("a built-in cannot be edited");
    assert_eq!(err.code, "forbidden");
    assert!(err.message.contains("Duplicate it"), "{err}");

    let err = lab
        .app
        .rpc(
            "compliance.policies.delete",
            json!({"policy_id": "builtin.ios.hardening"}),
        )
        .await
        .expect_err("a built-in cannot be deleted");
    assert_eq!(err.code, "forbidden");

    // Switching it off is allowed and survives.
    built_in["enabled"] = json!(false);
    let saved = lab
        .app
        .rpc("compliance.policies.save", json!({"policy": built_in}))
        .await
        .expect("the policy is switched off");
    assert_eq!(saved["enabled"], false);
    assert_eq!(
        saved["rules"].as_array().unwrap().len(),
        16,
        "it is unchanged"
    );
    assert!(
        audits(&lab.app, "compliance.policies.save")[0]
            .message
            .starts_with("Switched off")
    );

    // With both packs off there is nothing to run.
    let mut other = lab
        .app
        .rpc(
            "compliance.policies.get",
            json!({"policy_id": "builtin.routeros.hardening"}),
        )
        .await
        .expect("the other built-in");
    other["enabled"] = json!(false);
    lab.app
        .rpc("compliance.policies.save", json!({"policy": other}))
        .await
        .expect("it is switched off");
    let err = lab
        .app
        .rpc("compliance.run", json!({}))
        .await
        .expect_err("nothing would be judged");
    assert_eq!(err.code, "invalid");

    // Naming it explicitly runs it anyway.
    lab.app
        .store()
        .config_insert(&lab.id("SW1"), "running", WEAK, "manual", "")
        .expect("a stored configuration");
    let run = lab
        .app
        .rpc(
            "compliance.run",
            json!({"policy_ids": ["builtin.ios.hardening"]}),
        )
        .await
        .expect("the run");
    assert_eq!(
        run["summary"]["rules"], 16,
        "a policy named by id runs even when off"
    );
    assert_eq!(run["summary"]["fail"], 10);

    // And a copy of it is an ordinary policy that can be edited.
    let mut copy = lab
        .app
        .rpc(
            "compliance.policies.duplicate",
            json!({"policy_id": "builtin.ios.hardening", "name": "Ours"}),
        )
        .await
        .expect("the copy");
    assert_eq!(copy["builtin"], false);
    copy["rules"][0]["severity"] = json!("info");
    let saved = lab
        .app
        .rpc("compliance.policies.save", json!({"policy": copy}))
        .await
        .expect("the copy is saved");
    assert_eq!(saved["rules"][0]["severity"], "info");
}

#[tokio::test]
async fn a_policy_that_cannot_be_evaluated_is_refused_and_says_why() {
    let lab = lab(&["SW1"]).await;
    let broken = json!({
        "id": "",
        "name": "",
        "rules": [
            {"id": "a", "title": "", "check": {"type": "matches", "pattern": "(unclosed"}},
            {"id": "a", "title": "Twice", "check": {"type": "present", "line": "  "}},
            {"id": "b", "title": "Nothing asked", "check": {"type": "section", "parent": "^line vty"}},
            {"id": "c", "title": "Bad fix", "check": {"type": "absent", "line": "ip http server"},
             "remediation": ["ntp server {{ nonsense }}"]},
        ],
    });

    let checked = lab
        .app
        .rpc("compliance.policies.validate", json!({"policy": broken}))
        .await
        .expect("the policy is checked");
    assert_eq!(checked["ok"], false);
    let problems: Vec<(String, String, String)> = checked["problems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|problem| {
            (
                problem["rule_id"].as_str().unwrap().to_string(),
                problem["field"].as_str().unwrap().to_string(),
                problem["level"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    for wanted in [
        ("", "name", "error"),
        ("a", "check.pattern", "error"),
        ("a", "title", "error"),
        ("a", "id", "error"),
        ("a", "check.line", "error"),
        ("b", "check.must", "error"),
        ("c", "remediation", "error"),
        ("a", "rationale", "warning"),
    ] {
        let wanted = (
            wanted.0.to_string(),
            wanted.1.to_string(),
            wanted.2.to_string(),
        );
        assert!(
            problems.contains(&wanted),
            "{wanted:?} is missing from {problems:?}"
        );
    }
    assert_eq!(problems[0].2, "error", "errors come first");

    let err = lab
        .app
        .rpc("compliance.policies.save", json!({"policy": broken}))
        .await
        .expect_err("it cannot be saved");
    assert_eq!(err.code, "invalid");
    assert!(err.details.unwrap()["problems"].as_array().unwrap().len() >= 7);

    // A policy whose rules are sound passes, warnings and all.
    let checked = lab
        .app
        .rpc(
            "compliance.policies.validate",
            json!({"policy": policy("")}),
        )
        .await
        .expect("the policy is checked");
    assert_eq!(checked["ok"], true);
    assert_eq!(checked["problems"], json!([]));
}

#[tokio::test]
async fn a_run_is_stored_filtered_paged_and_charted() {
    let lab = lab(&["SW1", "SW2"]).await;
    lab.app
        .store()
        .config_insert(&lab.id("SW1"), "running", WEAK, "manual", "")
        .expect("a stored configuration");
    let mut events = support::Events::new(&lab.app);

    let run = lab
        .app
        .rpc("compliance.run", json!({}))
        .await
        .expect("the run");
    let run_id = run["run_id"].as_str().unwrap().to_string();
    let summary = &run["summary"];
    assert_eq!(summary["devices"], 2);
    assert_eq!(summary["devices_requested"], 2);
    assert_eq!(summary["devices_without_backup"], 1);
    assert_eq!(summary["rules"], 16);
    assert_eq!(
        summary["not_evaluated"], 16,
        "every rule of the device with no backup"
    );
    assert!(summary["fail"].as_u64().unwrap() > 0);
    assert!(summary["score_pct"].as_f64().unwrap() < 100.0);

    let finished = events.of("compliance.finished");
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0]["run_id"], run_id.as_str());
    assert_eq!(finished[0]["summary"]["fail"], summary["fail"]);
    assert_eq!(
        audits(&lab.app, "compliance.run")[0].body.as_ref().unwrap()["run_id"],
        run_id.as_str()
    );

    // Filtering by status, severity and device.
    let failures = lab
        .app
        .rpc(
            "compliance.results",
            json!({"run_id": run_id, "status": "fail", "limit": 500}),
        )
        .await
        .expect("the failures");
    let count = failures["findings"].as_array().unwrap().len();
    assert_eq!(count as u64, summary["fail"].as_u64().unwrap());
    assert!(
        failures["findings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|finding| finding["device_id"] == lab.id("SW1").as_str())
    );
    assert!(failures["next_cursor"].is_null());

    let critical = lab
        .app
        .rpc(
            "compliance.results",
            json!({"run_id": run_id, "severity": "critical", "status": "fail"}),
        )
        .await
        .expect("the critical failures");
    let titles: Vec<&str> = critical["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| finding["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        vec![
            "VTY lines accept SSH only",
            "No SNMP community is named `public` or `private`"
        ]
    );

    // Paging walks the whole run without repeating or losing a finding.
    let mut seen: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = lab
            .app
            .rpc(
                "compliance.results",
                json!({"run_id": run_id, "limit": 5, "cursor": cursor}),
            )
            .await
            .expect("a page");
        for finding in page["findings"].as_array().unwrap() {
            seen.push(format!("{}/{}", finding["device_id"], finding["rule_id"]));
        }
        match page["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!(seen.len(), 32, "16 rules on each of two devices");
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), seen.len());

    // A bad filter says what the values are.
    let err = lab
        .app
        .rpc("compliance.results", json!({"status": "passed"}))
        .await
        .expect_err("`passed` is not a status");
    assert_eq!(err.code, "invalid");
    assert!(err.message.contains("not_applicable"), "{err}");

    // The history has the run, and `compliance.results` without an id is the latest.
    let runs = lab
        .app
        .rpc("compliance.runs", json!({}))
        .await
        .expect("the history");
    assert_eq!(runs.as_array().unwrap().len(), 1);
    assert_eq!(runs[0]["run_id"], run_id.as_str());
    assert_eq!(runs[0]["has_findings"], true);

    lab.app
        .rpc("compliance.run", json!({"device_ids": [lab.id("SW1")]}))
        .await
        .expect("a second run");
    let runs = lab
        .app
        .rpc("compliance.runs", json!({}))
        .await
        .expect("the history");
    assert_eq!(runs.as_array().unwrap().len(), 2);
    assert_eq!(
        runs[0]["devices_requested"], 1,
        "the newest run comes first"
    );
    let latest = lab
        .app
        .rpc("compliance.results", json!({}))
        .await
        .expect("the latest results");
    assert_eq!(latest["summary"]["devices_requested"], 1);

    let err = lab
        .app
        .rpc("compliance.results", json!({"run_id": "nope"}))
        .await
        .expect_err("there is no such run");
    assert_eq!(err.code, "not_found");
}

#[tokio::test]
async fn remediation_renders_the_rules_lines_and_hands_them_to_the_change_pipeline() {
    let (lab, changes) = lab_with_changes(&["SW1", "SW2"]).await;
    for name in ["SW1", "SW2"] {
        lab.app
            .store()
            .config_insert(&lab.id(name), "running", WEAK, "manual", "")
            .expect("a stored configuration");
    }
    lab.app
        .rpc("compliance.policies.save", json!({"policy": policy("")}))
        .await
        .expect("the site policy is saved");

    let run = lab
        .app
        .rpc("compliance.run", json!({}))
        .await
        .expect("the run");
    let run_id = run["run_id"].as_str().unwrap().to_string();

    let handed = lab
        .app
        .rpc(
            "compliance.remediate",
            json!({
                "run_id": run_id,
                "findings": [
                    {"device_id": lab.id("SW1"), "rule_id": "ios.vty-transport-ssh"},
                    {"device_id": lab.id("SW1"), "rule_id": "site.ntp"},
                    {"device_id": lab.id("SW2"), "rule_id": "ios.no-source-route"},
                ],
            }),
        )
        .await
        .expect("the fix is handed over");
    assert_eq!(handed["change_id"], "change-1");
    assert_eq!(handed["devices"], 2);

    let call = changes.last();
    assert_eq!(call["origin"], "compliance");
    assert!(
        call["title"].as_str().unwrap().contains("compliance"),
        "{call}"
    );
    assert!(
        call["description"]
            .as_str()
            .unwrap()
            .contains("VTY lines accept SSH only"),
        "{call}"
    );
    assert_eq!(call["targets"].as_array().unwrap().len(), 2);
    assert_eq!(call["targets"][0]["intent"]["type"], "lines");

    // A rule written per section is rendered once for each offending section,
    // with the section header in front of its lines.
    assert_eq!(
        changes.lines_for(&lab.id("SW1")),
        vec![
            "line vty 0 4",
            " transport input ssh",
            "line vty 5 15",
            " transport input ssh",
            "ntp server 192.0.2.1",
        ]
    );
    assert_eq!(
        changes.lines_for(&lab.id("SW2")),
        vec!["no ip source-route"]
    );
    assert_eq!(audits(&lab.app, "compliance.remediate").len(), 1);

    // A rule that carries no remediation says so rather than inventing one.
    let err = lab
        .app
        .rpc(
            "compliance.remediate",
            json!({
                "run_id": run_id,
                "findings": [{"device_id": lab.id("SW1"), "rule_id": "ios.enable-secret"}],
            }),
        )
        .await
        .expect_err("there is nothing to send");
    assert_eq!(err.code, "invalid");
    assert!(err.message.contains("carries no remediation"), "{err}");

    // A finding that did not fail is not something to fix.
    let err = lab
        .app
        .rpc(
            "compliance.remediate",
            json!({
                "run_id": run_id,
                "findings": [{"device_id": lab.id("SW1"), "rule_id": "ios.no-http-server"}],
            }),
        )
        .await
        .expect_err("that rule passed");
    assert_eq!(err.code, "not_found");

    let err = lab
        .app
        .rpc("compliance.remediate", json!({"findings": []}))
        .await
        .expect_err("nothing was named");
    assert_eq!(err.code, "invalid");
}

#[tokio::test]
async fn without_the_change_pipeline_the_fix_is_refused_clearly() {
    let lab = lab(&["SW1"]).await;
    lab.app
        .store()
        .config_insert(&lab.id("SW1"), "running", WEAK, "manual", "")
        .expect("a stored configuration");
    let run = lab
        .app
        .rpc("compliance.run", json!({}))
        .await
        .expect("the run");

    let err = lab
        .app
        .rpc(
            "compliance.remediate",
            json!({
                "run_id": run["run_id"],
                "findings": [{"device_id": lab.id("SW1"), "rule_id": "ios.vty-transport-ssh"}],
            }),
        )
        .await
        .expect_err("there is no change pipeline in this build");
    assert_eq!(err.code, "unavailable");
    assert!(err.message.contains("changes.create"), "{err}");
    assert!(err.message.contains("Nothing was changed"), "{err}");
    // And nothing was recorded as if it had happened.
    assert!(audits(&lab.app, "compliance.remediate").is_empty());
}

#[tokio::test]
async fn an_error_from_the_change_pipeline_is_passed_on_as_it_is() {
    let (lab, changes) = lab_with_changes(&["SW1"]).await;
    lab.app
        .store()
        .config_insert(&lab.id("SW1"), "running", WEAK, "manual", "")
        .expect("a stored configuration");
    let run = lab
        .app
        .rpc("compliance.run", json!({}))
        .await
        .expect("the run");
    changes.fail_with("forbidden", "`SW1` is protected and needs a named approver");

    let err = lab
        .app
        .rpc(
            "compliance.remediate",
            json!({
                "run_id": run["run_id"],
                "findings": [{"device_id": lab.id("SW1"), "rule_id": "ios.vty-transport-ssh"}],
            }),
        )
        .await
        .expect_err("the change pipeline refused");
    assert_eq!(err.code, "forbidden");
    assert!(err.message.contains("named approver"), "{err}");
}

#[tokio::test]
async fn testing_a_rule_against_a_device_shows_the_fix_it_would_send() {
    let lab = lab(&["SW1"]).await;
    lab.app
        .store()
        .config_insert(&lab.id("SW1"), "running", WEAK, "manual", "")
        .expect("a stored configuration");

    let built_in = lab
        .app
        .rpc(
            "compliance.policies.get",
            json!({"policy_id": "builtin.ios.hardening"}),
        )
        .await
        .expect("the pack");
    let rule = built_in["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|rule| rule["id"] == "ios.vty-transport-ssh")
        .unwrap()
        .clone();

    let tested = lab
        .app
        .rpc(
            "compliance.test",
            json!({"rule": rule, "device_id": lab.id("SW1")}),
        )
        .await
        .expect("the rule runs");
    assert_eq!(tested["finding"]["status"], "fail");
    assert_eq!(tested["finding"]["device_name"], "SW1");
    assert_eq!(tested["finding"]["remediable"], true);
    assert_eq!(tested["syntax"], "ios");
    assert_eq!(
        tested["fix"],
        json!([
            "line vty 0 4",
            " transport input ssh",
            "line vty 5 15",
            " transport input ssh"
        ])
    );

    // Against pasted text there is no device to render the fix for.
    let tested = lab
        .app
        .rpc("compliance.test", json!({"rule": rule, "text": WEAK}))
        .await
        .expect("the rule runs");
    assert_eq!(tested["finding"]["status"], "fail");
    assert_eq!(tested["fix"], json!([]));
    assert_eq!(tested["finding"]["device_id"], "");
}

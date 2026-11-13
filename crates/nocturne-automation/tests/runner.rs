//! The command runner, through the RPC surface it is written against.

mod common;

use common::{lab, results, state_of};
use serde_json::{Value, json};

/// A device type like the lab's, but with an ntc-templates platform, so the
/// parsing path has something to find.
const IOS_WITH_NTC: &str = r#"
id: test_ios_ntc
name: Test IOS (ntc)
vendor: Test
os: IOS
ntc_platform: cisco_ios
cli:
  prompt: '^(?P<host>[\w.\-]{1,63})(\([\w\-/. ]+\))?[>#]\s?$'
  user_prompt: '^[\w.\-]{1,63}>\s?$'
  config_prompt: '^[\w.\-]{1,63}\([\w\-/. ]+\)#\s?$'
  enable: {}
  init: ['terminal length 0']
  errors: ['^% ']
config:
  syntax: ios
  enter: ['configure terminal']
  exit: ['end']
  save: ['write memory']
  running: 'show running-config'
policy:
  read_only: ['^show ', '^ping ', '^terminal (length|width) ']
  dangerous: ['^reload', '^erase ']
"#;

#[tokio::test]
async fn a_run_records_every_device_it_selected_including_the_ones_it_could_not_reach() {
    let lab = lab(&["SW1", "SW2"]).await;
    // A device in the inventory whose lab node does not exist: the session
    // cannot be opened, which is what an unreachable switch looks like.
    lab.app
        .rpc(
            "devices.create",
            json!({"name": "GHOST", "profile": "test_ios", "site": "Lab",
                   "credential_id": lab.credential_id, "lab": {"lab_id": "testlab", "node": "NOPE"}}),
        )
        .await
        .expect("the ghost device is created");

    let started = lab
        .app
        .rpc(
            "runner.run",
            json!({"filter": {"site": "Lab"}, "commands": ["show version"], "name": "Versions"}),
        )
        .await
        .expect("the run starts");
    let run_id = started["run_id"].as_str().expect("a run id").to_string();
    let job = common::finished(&lab.app, &started).await;
    assert_eq!(job.state, nocturne_app::jobs::JobState::Succeeded);

    let results = results(&lab.app, &run_id).await;
    assert_eq!(results.len(), 3, "a selected device always has a result");
    assert_eq!(state_of(&results, "SW1"), "ok");
    assert_eq!(state_of(&results, "SW2"), "ok");
    assert_eq!(
        state_of(&results, "GHOST"),
        "unreached",
        "a device that could not be reached is `unreached`, not missing"
    );
    let ghost = results
        .iter()
        .find(|result| result["name"] == json!("GHOST"))
        .expect("the ghost's result");
    assert!(
        ghost["error"]
            .as_str()
            .is_some_and(|error| !error.is_empty()),
        "it says why: {ghost:?}"
    );
    assert!(ghost["outputs"].as_array().expect("outputs").is_empty());

    let run = lab
        .app
        .rpc("runner.get", json!({"run_id": run_id}))
        .await
        .expect("runner.get")["run"]
        .clone();
    assert_eq!(
        (
            run["devices"].as_u64(),
            run["ok"].as_u64(),
            run["unreached"].as_u64(),
            run["failed"].as_u64()
        ),
        (Some(3), Some(2), Some(1), Some(0)),
        "the tally adds up to the devices selected: {run:?}"
    );
    // The reachable devices really were asked.
    assert!(
        lab.fake("SW1")
            .seen()
            .iter()
            .any(|line| line == "show version")
    );
}

#[tokio::test]
async fn one_device_that_rejects_a_command_does_not_spoil_the_others() {
    let lab = lab(&["SW1", "SW2"]).await;
    // `show frobnicate` is a read by policy; this device simply does not have it.
    lab.fake("SW2")
        .with(|state| state.outputs.remove("show frobnicate"));
    let started = lab
        .app
        .rpc(
            "runner.run",
            json!({"filter": {}, "commands": ["show version", "show frobnicate"]}),
        )
        .await
        .expect("the run starts");
    let run_id = started["run_id"].as_str().expect("run id").to_string();
    common::finished(&lab.app, &started).await;

    let results = results(&lab.app, &run_id).await;
    assert_eq!(results.len(), 2);
    for name in ["SW1", "SW2"] {
        assert_eq!(state_of(&results, name), "failed", "{name}");
    }
    let first = &results[0];
    assert_eq!(
        first["outputs"].as_array().expect("outputs").len(),
        2,
        "what did work is kept: {first:?}"
    );
    assert!(
        first["outputs"][0]["output"]
            .as_str()
            .expect("output")
            .contains("Cisco IOS"),
        "the first command's output is there"
    );
    assert!(
        first["outputs"][1]["error"]
            .as_str()
            .expect("an error")
            .contains("Invalid input"),
        "and the device's own words for the second: {first:?}"
    );
}

#[tokio::test]
async fn a_command_that_is_not_a_read_refuses_the_whole_run_before_anything_is_sent() {
    let lab = lab(&["SW1", "SW2"]).await;
    let refused = lab
        .app
        .rpc(
            "runner.run",
            json!({"filter": {}, "commands": ["show version", "reload in 5"]}),
        )
        .await
        .expect_err("not a read");
    assert_eq!(refused.code, "forbidden");
    assert!(
        refused.message.contains("reload in 5") && refused.message.contains("Nothing was sent"),
        "{}",
        refused.message
    );
    // Not one device was touched, not even the first.
    for name in ["SW1", "SW2"] {
        assert!(
            lab.fake(name).seen().is_empty(),
            "{name} was sent something"
        );
    }
    assert!(
        lab.app
            .rpc("runner.runs", json!({}))
            .await
            .expect("runs")
            .as_array()
            .expect("array")
            .is_empty(),
        "a refused run leaves no record because it never started"
    );
}

#[tokio::test]
async fn a_dry_run_says_what_would_happen_and_sends_nothing() {
    let lab = lab(&["SW1"]).await;
    let started = lab
        .app
        .rpc(
            "runner.run",
            json!({"device_ids": [lab.id("SW1")], "commands": ["show version"], "dry_run": true}),
        )
        .await
        .expect("the dry run starts");
    let run_id = started["run_id"].as_str().expect("run id").to_string();
    common::finished(&lab.app, &started).await;
    let results = results(&lab.app, &run_id).await;
    assert_eq!(state_of(&results, "SW1"), "ok");
    assert!(
        results[0]["reason"]
            .as_str()
            .expect("a reason")
            .contains("would send 1 command"),
        "{:?}",
        results[0]
    );
    assert!(lab.fake("SW1").seen().is_empty(), "nothing was sent");
    let run = lab
        .app
        .rpc("runner.get", json!({"run_id": run_id}))
        .await
        .expect("runner.get")["run"]
        .clone();
    assert_eq!(run["dry_run"], json!(true));
}

#[tokio::test]
async fn a_device_already_in_another_run_is_skipped_and_says_which_run_has_it() {
    let lab = lab(&["SW1", "SW2"]).await;
    let state = lab
        .app
        .state::<nocturne_automation::state::AutomationState>();
    // Exactly what a run in flight does to the device it is working on.
    let lease = state
        .claim(&lab.id("SW1"), "run-earlier", "Nightly health check")
        .expect("the first claim");

    let started = lab
        .app
        .rpc(
            "runner.run",
            json!({"filter": {}, "commands": ["show version"]}),
        )
        .await
        .expect("the run starts");
    let run_id = started["run_id"].as_str().expect("run id").to_string();
    common::finished(&lab.app, &started).await;
    drop(lease);

    let results = results(&lab.app, &run_id).await;
    assert_eq!(results.len(), 2, "the busy device is still in the run");
    assert_eq!(state_of(&results, "SW2"), "ok");
    assert_eq!(state_of(&results, "SW1"), "skipped");
    let skipped = results
        .iter()
        .find(|result| result["name"] == json!("SW1"))
        .expect("SW1");
    assert!(
        skipped["reason"]
            .as_str()
            .expect("a reason")
            .contains("Nightly health check"),
        "the reason names the run that has it: {skipped:?}"
    );
    assert!(
        lab.fake("SW1").seen().is_empty(),
        "no second session was opened on the busy device"
    );
}

#[tokio::test]
async fn compare_groups_devices_by_output_and_ignores_the_lines_that_always_differ() {
    let lab = lab(&["SW1", "SW2", "SW3"]).await;
    // Two agree; the third differs. Every one has its own uptime line, which
    // is exactly the kind of difference nobody wants to be told about.
    for (name, server, uptime) in [
        ("SW1", "192.0.2.1", "1 day"),
        ("SW2", "192.0.2.1", "7 weeks"),
        ("SW3", "192.0.2.9", "3 hours"),
    ] {
        lab.fake(name).with(|state| {
            state.outputs.insert(
                "show ntp associations".into(),
                format!("SW uptime is {uptime}\n  address ref clock\n*~{server} .GPS.\n"),
            );
        });
    }
    let started = lab
        .app
        .rpc(
            "runner.run",
            json!({"filter": {}, "commands": ["show ntp associations"]}),
        )
        .await
        .expect("the run starts");
    let run_id = started["run_id"].as_str().expect("run id").to_string();
    common::finished(&lab.app, &started).await;

    let comparison = lab
        .app
        .rpc(
            "runner.compare",
            json!({"run_id": run_id, "command": "show ntp associations"}),
        )
        .await
        .expect("runner.compare");
    assert_eq!(comparison["devices"], json!(3));
    let groups = comparison["groups"].as_array().expect("groups");
    assert_eq!(groups.len(), 2, "the uptime lines did not split the group");
    let majority: Vec<&str> = groups[0]["devices"]
        .as_array()
        .expect("members")
        .iter()
        .map(|member| member["name"].as_str().expect("name"))
        .collect();
    assert_eq!(majority, vec!["SW1", "SW2"], "largest group first");
    let diff = groups[1]["diff"].as_array().expect("a diff");
    let removed: Vec<&str> = diff
        .iter()
        .filter(|line| line["op"] == json!("-"))
        .map(|line| line["text"].as_str().expect("text"))
        .collect();
    let added: Vec<&str> = diff
        .iter()
        .filter(|line| line["op"] == json!("+"))
        .map(|line| line["text"].as_str().expect("text"))
        .collect();
    assert_eq!(removed, vec!["*~192.0.2.1 .GPS."]);
    assert_eq!(added, vec!["*~192.0.2.9 .GPS."]);
    assert!(
        comparison["missing"]
            .as_array()
            .expect("missing")
            .is_empty(),
        "every device answered"
    );
}

#[tokio::test]
async fn compare_says_which_devices_have_no_output_to_compare() {
    let lab = lab(&["SW1"]).await;
    lab.app
        .rpc(
            "devices.create",
            json!({"name": "GHOST", "profile": "test_ios", "credential_id": lab.credential_id,
                   "lab": {"lab_id": "testlab", "node": "NOPE"}}),
        )
        .await
        .expect("the ghost");
    let started = lab
        .app
        .rpc(
            "runner.run",
            json!({"filter": {}, "commands": ["show version"]}),
        )
        .await
        .expect("the run starts");
    let run_id = started["run_id"].as_str().expect("run id").to_string();
    common::finished(&lab.app, &started).await;

    let comparison = lab
        .app
        .rpc(
            "runner.compare",
            json!({"run_id": run_id, "command": "show version"}),
        )
        .await
        .expect("runner.compare");
    let missing = comparison["missing"].as_array().expect("missing");
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0]["name"], json!("GHOST"));
    assert_eq!(missing[0]["state"], json!("unreached"));
}

#[tokio::test]
async fn parsing_uses_the_device_types_template_and_is_absent_when_there_is_none() {
    let lab = lab(&["SW1"]).await;
    nocturne_app::profiles::save(&lab.app, IOS_WITH_NTC).expect("the profile loads");
    lab.app
        .rpc(
            "devices.create",
            json!({"name": "SW1-NTC", "profile": "test_ios_ntc", "credential_id": lab.credential_id,
                   "lab": {"lab_id": "testlab", "node": "SW1"}}),
        )
        .await
        .expect("a device of the parsing type");

    let started = lab
        .app
        .rpc(
            "runner.run",
            json!({"filter": {}, "commands": ["show version"], "parse": true}),
        )
        .await
        .expect("the run starts");
    let run_id = started["run_id"].as_str().expect("run id").to_string();
    common::finished(&lab.app, &started).await;
    let results = results(&lab.app, &run_id).await;

    let with_template = results
        .iter()
        .find(|result| result["name"] == json!("SW1-NTC"))
        .expect("the parsing device");
    let parsed = with_template["outputs"][0]["parsed"]
        .as_array()
        .expect("rows, because cisco_ios has a show-version template");
    assert_eq!(parsed.len(), 1);
    assert!(
        parsed[0]
            .as_object()
            .expect("a row")
            .values()
            .any(|value| value.as_str() == Some("15.2(4)E10")),
        "the row carries what the device said: {:?}",
        parsed[0]
    );

    let without = results
        .iter()
        .find(|result| result["name"] == json!("SW1"))
        .expect("the plain device");
    assert_eq!(
        without["outputs"][0].get("parsed"),
        None,
        "no template is `parsed` absent, not an empty list: {without:?}"
    );
}

#[tokio::test]
async fn output_longer_than_the_cap_is_cut_and_says_so() {
    let lab = lab(&["SW1"]).await;
    lab.app
        .rpc(
            "settings.set",
            json!({"section": "automation", "value": {"output_cap_bytes": 4096}}),
        )
        .await
        .expect("the cap is set");
    lab.fake("SW1").with(|state| {
        state
            .outputs
            .insert("show tech-support".into(), "x".repeat(20_000));
    });
    let started = lab
        .app
        .rpc(
            "runner.run",
            json!({"device_ids": [lab.id("SW1")], "commands": ["show tech-support"]}),
        )
        .await
        .expect("the run starts");
    let run_id = started["run_id"].as_str().expect("run id").to_string();
    common::finished(&lab.app, &started).await;
    let results = results(&lab.app, &run_id).await;
    let output = &results[0]["outputs"][0];
    assert_eq!(output["truncated"], json!(true));
    let text = output["output"].as_str().expect("output");
    assert!(text.len() < 5000, "it really was cut: {} bytes", text.len());
    assert!(text.contains("were not kept"), "and it says so");
}

#[tokio::test]
async fn search_finds_lines_per_device_as_text_or_as_a_regexp() {
    let lab = lab(&["SW1", "SW2"]).await;
    let started = lab
        .app
        .rpc(
            "runner.run",
            json!({"filter": {}, "commands": ["show version"]}),
        )
        .await
        .expect("the run starts");
    let run_id = started["run_id"].as_str().expect("run id").to_string();
    common::finished(&lab.app, &started).await;

    let hits = lab
        .app
        .rpc(
            "runner.search",
            json!({"run_id": run_id, "query": "Version 15.2"}),
        )
        .await
        .expect("runner.search");
    let hits = hits.as_array().expect("matches");
    assert_eq!(hits.len(), 2, "both devices say it");
    assert_eq!(hits[0]["command"], json!("show version"));
    assert!(hits[0]["line_no"].as_u64().expect("a line number") >= 1);

    let regex = lab
        .app
        .rpc(
            "runner.search",
            json!({"run_id": run_id, "query": "^SW\\d uptime", "regex": true}),
        )
        .await
        .expect("a regexp search");
    assert_eq!(regex.as_array().expect("matches").len(), 2);

    let bad = lab
        .app
        .rpc(
            "runner.search",
            json!({"run_id": run_id, "query": "([", "regex": true}),
        )
        .await
        .expect_err("not a regexp");
    assert_eq!(bad.code, "invalid");
}

#[tokio::test]
async fn a_run_exports_as_text_json_or_csv() {
    let lab = lab(&["SW1"]).await;
    let started = lab
        .app
        .rpc(
            "runner.run",
            json!({"filter": {}, "commands": ["show version"], "name": "Versions"}),
        )
        .await
        .expect("the run starts");
    let run_id = started["run_id"].as_str().expect("run id").to_string();
    common::finished(&lab.app, &started).await;

    for format in ["text", "json", "csv"] {
        let exported = lab
            .app
            .rpc("runner.export", json!({"run_id": run_id, "format": format}))
            .await
            .unwrap_or_else(|err| panic!("{format}: {err}"));
        let text = exported["text"].as_str().expect("text");
        assert!(text.contains("SW1"), "{format} names the device");
        assert!(
            text.contains("Cisco IOS") || text.contains("Cisco"),
            "{format} carries the output"
        );
    }
    assert!(
        serde_json::from_str::<Value>(
            lab.app
                .rpc("runner.export", json!({"run_id": run_id, "format": "json"}))
                .await
                .expect("json")["text"]
                .as_str()
                .expect("text")
        )
        .is_ok(),
        "the JSON export is JSON"
    );
    assert_eq!(
        lab.app
            .rpc("runner.export", json!({"run_id": run_id, "format": "pdf"}))
            .await
            .expect_err("no such format")
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn history_keeps_the_newest_runs_and_delete_removes_their_outputs_too() {
    let lab = lab(&["SW1"]).await;
    lab.app
        .rpc(
            "settings.set",
            json!({"section": "automation", "value": {"keep_runs": 2}}),
        )
        .await
        .expect("retention is set");
    let mut ids = Vec::new();
    for index in 0..3 {
        let started = lab
            .app
            .rpc(
                "runner.run",
                json!({"filter": {}, "commands": ["show version"], "name": format!("run {index}")}),
            )
            .await
            .expect("the run starts");
        ids.push(started["run_id"].as_str().expect("run id").to_string());
        common::finished(&lab.app, &started).await;
    }
    let runs = lab
        .app
        .rpc("runner.runs", json!({}))
        .await
        .expect("runner.runs");
    let runs = runs.as_array().expect("array");
    assert_eq!(runs.len(), 2, "the oldest run was pruned");
    assert_eq!(runs[0]["name"], json!("run 2"), "newest first");
    assert_eq!(
        lab.app
            .rpc("runner.get", json!({"run_id": ids[0]}))
            .await
            .expect_err("gone")
            .code,
        "not_found"
    );

    // Deleting a run takes its outputs with it.
    assert!(!results(&lab.app, &ids[2]).await.is_empty());
    let deleted = lab
        .app
        .rpc("runner.delete", json!({"run_ids": [ids[2].clone()]}))
        .await
        .expect("runner.delete");
    assert_eq!(deleted["removed"], json!(1));
    assert_eq!(
        lab.app
            .rpc("runner.get", json!({"run_id": ids[2]}))
            .await
            .expect_err("the run is gone")
            .code,
        "not_found"
    );
    assert_eq!(
        lab.app
            .rpc("runner.runs", json!({}))
            .await
            .expect("runs")
            .as_array()
            .expect("array")
            .len(),
        1
    );
}

#[tokio::test]
async fn a_selection_that_matches_nothing_is_refused_rather_than_run_empty() {
    let lab = lab(&["SW1"]).await;
    let err = lab
        .app
        .rpc(
            "runner.run",
            json!({"filter": {"site": "Nowhere"}, "commands": ["show version"]}),
        )
        .await
        .expect_err("nothing matches");
    assert_eq!(err.code, "not_found");
    assert!(err.message.contains("nothing would run"), "{}", err.message);

    let err = lab
        .app
        .rpc("runner.run", json!({"commands": ["show version"]}))
        .await
        .expect_err("no devices named");
    assert_eq!(err.code, "invalid");

    let err = lab
        .app
        .rpc("runner.run", json!({"filter": {}, "commands": []}))
        .await
        .expect_err("no commands");
    assert_eq!(err.code, "invalid");

    let err = lab
        .app
        .rpc(
            "runner.run",
            json!({"device_ids": ["no-such-device"], "commands": ["show version"]}),
        )
        .await
        .expect_err("a typo must not shrink the run");
    assert_eq!(err.code, "not_found");
}

#[tokio::test]
async fn a_filter_chooses_devices_when_the_run_starts_not_when_it_was_written() {
    let lab = lab(&["SW1"]).await;
    lab.app
        .rpc(
            "devices.update",
            json!({"id": lab.id("SW1"), "role": "access", "tags": ["campus", "floor-1"]}),
        )
        .await
        .expect("SW1 is an access switch");

    let chosen = lab
        .app
        .rpc(
            "tasks.devices",
            json!({"filter": {"role": "access", "tags": ["campus"]}}),
        )
        .await
        .expect("tasks.devices");
    assert_eq!(chosen["count"], json!(1));

    // A second access switch appears; the same filter now means two.
    let second = lab
        .app
        .rpc(
            "devices.create",
            json!({"name": "SW9", "address": "10.0.0.9", "role": "access", "tags": ["campus"]}),
        )
        .await
        .expect("SW9");
    assert!(second["id"].is_string());
    let chosen = lab
        .app
        .rpc(
            "tasks.devices",
            json!({"filter": {"role": "access", "tags": ["campus"]}}),
        )
        .await
        .expect("tasks.devices");
    assert_eq!(chosen["count"], json!(2));

    // And an exclusion takes one back out.
    let chosen = lab
        .app
        .rpc(
            "tasks.devices",
            json!({"filter": {"role": "access"}, "exclude_device_ids": ["SW9"]}),
        )
        .await
        .expect("tasks.devices");
    assert_eq!(chosen["count"], json!(1));
    assert_eq!(chosen["devices"][0]["name"], json!("SW1"));
}

#[tokio::test]
async fn cancelling_a_run_stops_it_and_the_devices_it_never_reached_say_so() {
    let lab = lab(&["SW1", "SW2", "SW3", "SW4"]).await;
    lab.app
        .rpc(
            "settings.set",
            json!({"section": "automation", "value": {"concurrency": 1}}),
        )
        .await
        .expect("one at a time");
    let started = lab
        .app
        .rpc(
            "runner.run",
            json!({"filter": {}, "commands": ["show version", "show running-config"]}),
        )
        .await
        .expect("the run starts");
    let run_id = started["run_id"].as_str().expect("run id").to_string();
    let job_id = started["job_id"].as_str().expect("job id").to_string();
    // Cancel at once: with one device at a time, most of them never start.
    lab.app
        .rpc("jobs.cancel", json!({"id": job_id}))
        .await
        .expect("jobs.cancel");
    let job = common::finished(&lab.app, &started).await;
    assert_eq!(job.state, nocturne_app::jobs::JobState::Cancelled);

    let results = results(&lab.app, &run_id).await;
    assert_eq!(results.len(), 4, "every selected device still has a result");
    let skipped: Vec<&Value> = results
        .iter()
        .filter(|result| result["state"] == json!("skipped"))
        .collect();
    assert!(
        !skipped.is_empty(),
        "the devices the cancel beat are skipped, not forgotten: {results:?}"
    );
    assert!(
        skipped.iter().all(|result| result["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("stopped"))),
        "and they say why: {skipped:?}"
    );
    let run = lab
        .app
        .rpc("runner.get", json!({"run_id": run_id}))
        .await
        .expect("runner.get")["run"]
        .clone();
    assert_eq!(run["cancelled"], json!(true));
}

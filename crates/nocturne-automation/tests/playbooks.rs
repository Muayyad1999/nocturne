//! Playbooks: validation, every step kind, failure isolation, and the one
//! change a run's template steps add up to.

mod common;

use common::{Backups, Changes, lab, lab_at, results, state_of, until};
use nocturne_automation::state::AutomationState;
use serde_json::{Value, json};

fn steps_of(result: &Value) -> Vec<(String, String, String)> {
    result["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .map(|step| {
            (
                step["name"].as_str().unwrap_or_default().to_string(),
                step["state"].as_str().unwrap_or_default().to_string(),
                step["detail"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn validation_says_what_is_wrong_and_in_which_step() {
    let lab = lab(&["SW1"]).await;
    let checked = lab
        .app
        .rpc(
            "playbooks.validate",
            json!({"yaml": "name: Bad\nsteps:\n  - name: two things\n    collect: [facts]\n    \
                   commands: [show version]\n  - name: neither\n    register: x\n  - name: nope\n    \
                   commands: [reload in 5]\n  - name: bad expression\n    assert:\n      \
                   that: \"interfaces | nonsense(\"\n  - name: no such template\n    template:\n      \
                   id: not_a_template\n"}),
        )
        .await
        .expect("playbooks.validate");
    assert_eq!(checked["ok"], json!(false));
    let problems = checked["problems"].as_array().expect("problems");
    let at = |step: u64| -> String {
        problems
            .iter()
            .filter(|problem| problem["step"] == json!(step))
            .map(|problem| problem["message"].as_str().unwrap_or_default().to_string())
            .collect::<Vec<_>>()
            .join(" | ")
    };
    assert!(at(1).contains("exactly one thing"), "step 1: {}", at(1));
    assert!(at(2).contains("exactly one thing"), "step 2: {}", at(2));
    assert!(
        at(3).contains("not a read-only command") && at(3).contains("becomes a change"),
        "step 3: {}",
        at(3)
    );
    assert!(at(4).contains("does not compile"), "step 4: {}", at(4));
    assert!(
        at(5).contains("no template `not_a_template`"),
        "step 5: {}",
        at(5)
    );

    // An unknown key is a problem with no step number: the YAML did not parse.
    let checked = lab
        .app
        .rpc(
            "playbooks.validate",
            json!({"yaml": "name: Bad\nsteps:\n  - name: x\n    collect: [facts]\n    whoops: true\n"}),
        )
        .await
        .expect("playbooks.validate");
    assert_eq!(checked["ok"], json!(false));
    assert!(
        checked["problems"][0]["message"]
            .as_str()
            .expect("a message")
            .contains("whoops"),
        "{checked:?}"
    );
    assert!(checked["problems"][0]["step"].is_null());

    // And an unrunnable playbook cannot be saved or run.
    assert_eq!(
        lab.app
            .rpc(
                "playbooks.save",
                json!({"name": "Bad", "yaml": "steps:\n  - commands: [reload]\n"})
            )
            .await
            .expect_err("not saved")
            .code,
        "invalid"
    );
    assert_eq!(
        lab.app
            .rpc(
                "playbooks.run",
                json!({"yaml": "steps:\n  - commands: [reload]\n", "filter": {}})
            )
            .await
            .expect_err("not run")
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn every_step_kind_does_its_part_and_the_run_records_each_one() {
    let lab = lab(&["SW1"]).await;
    let yaml = r#"
name: Everything
vars:
  wanted: 1
steps:
  - name: Collect state
    collect: [facts, interfaces]
  - name: Save evidence
    commands: [show version]
    register: evidence
  - name: There is evidence
    assert:
      that: "evidence | length >= vars.wanted"
      message: "{{ device.name }} produced nothing"
  - name: Note the site
    set:
      site_tag: "device.site | lower"
  - name: The site was noted
    assert:
      that: "site_tag == 'lab'"
  - name: Back up
    backup: true
  - name: Only for SW9
    when: "device.name == 'SW9'"
    commands: [show clock]
  - name: Settle
    pause: { seconds: 0 }
"#;
    let started = lab
        .app
        .rpc(
            "playbooks.run",
            json!({"yaml": yaml, "device_ids": [lab.id("SW1")]}),
        )
        .await
        .expect("playbooks.run");
    let run_id = started["run_id"].as_str().expect("run id").to_string();
    let job = common::finished(&lab.app, &started).await;
    assert_eq!(
        job.state,
        nocturne_app::jobs::JobState::Succeeded,
        "{:?}",
        job.error
    );

    let results = results(&lab.app, &run_id).await;
    assert_eq!(state_of(&results, "SW1"), "ok");
    let steps = steps_of(&results[0]);
    let names: Vec<&str> = steps.iter().map(|(name, ..)| name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "Collect state",
            "Save evidence",
            "There is evidence",
            "Note the site",
            "The site was noted",
            "Back up",
            "Only for SW9",
            "Settle"
        ]
    );
    let states: Vec<&str> = steps.iter().map(|(_, state, _)| state.as_str()).collect();
    assert_eq!(
        states,
        vec!["ok", "ok", "ok", "ok", "ok", "ok", "skipped", "ok"],
        "{steps:?}"
    );
    assert!(
        steps[6].2.contains("is not true here"),
        "the skipped step says why: {:?}",
        steps[6]
    );

    // `collect` really collected, `commands` really read, `backup` really asked.
    assert!(
        nocturne_app::collect::latest(
            &lab.app,
            &lab.id("SW1"),
            nocturne_drivers::model::Kind::Facts
        )
        .expect("collect::latest")
        .is_some(),
        "the collect step stored facts"
    );
    let evidence = results[0]["steps"][1]["outputs"][0]["output"]
        .as_str()
        .expect("the command's output");
    assert!(evidence.contains("Cisco IOS"), "{evidence}");
    assert_eq!(
        lab.app
            .state::<Backups>()
            .0
            .lock()
            .expect("lock")
            .as_slice(),
        &[lab.id("SW1")]
    );
    assert!(
        !lab.fake("SW1")
            .seen()
            .iter()
            .any(|line| line == "show clock"),
        "the `when` step was not run"
    );
}

#[tokio::test]
async fn a_failed_assert_stops_that_device_and_leaves_the_others_alone() {
    let lab = lab(&["SW1", "SW2"]).await;
    let yaml = r#"
name: Only SW1 may pass
steps:
  - name: Uplinks are up
    assert:
      that: "device.name == 'SW1'"
      message: "{{ device.name }} is not the one we wanted"
  - name: Read the version
    commands: [show version]
"#;
    let started = lab
        .app
        .rpc("playbooks.run", json!({"yaml": yaml, "filter": {}}))
        .await
        .expect("playbooks.run");
    let run_id = started["run_id"].as_str().expect("run id").to_string();
    common::finished(&lab.app, &started).await;

    let results = results(&lab.app, &run_id).await;
    assert_eq!(state_of(&results, "SW1"), "ok");
    assert_eq!(state_of(&results, "SW2"), "failed");
    let failed = results
        .iter()
        .find(|result| result["name"] == json!("SW2"))
        .expect("SW2");
    assert!(
        failed["error"]
            .as_str()
            .expect("an error")
            .contains("SW2 is not the one we wanted"),
        "the assert's own message, rendered: {failed:?}"
    );
    let steps = steps_of(failed);
    assert_eq!(
        steps.len(),
        1,
        "the device stopped at the assert: {steps:?}"
    );
    assert!(
        lab.fake("SW1")
            .seen()
            .iter()
            .any(|line| line == "show version"),
        "SW1 carried on"
    );
    assert!(
        !lab.fake("SW2")
            .seen()
            .iter()
            .any(|line| line == "show version"),
        "SW2 did not"
    );

    let run = lab
        .app
        .rpc("playbooks.run_get", json!({"run_id": run_id}))
        .await
        .expect("playbooks.run_get")["run"]
        .clone();
    assert_eq!(
        (run["ok"].as_u64(), run["failed"].as_u64()),
        (Some(1), Some(1))
    );
}

#[tokio::test]
async fn ignore_errors_lets_the_device_carry_on_and_says_it_was_ignored() {
    let lab = lab(&["SW1"]).await;
    let yaml = r#"
name: Tolerant
steps:
  - name: Something untrue
    ignore_errors: true
    assert:
      that: "1 == 2"
  - name: Read the version
    commands: [show version]
"#;
    let started = lab
        .app
        .rpc(
            "playbooks.run",
            json!({"yaml": yaml, "device_ids": [lab.id("SW1")]}),
        )
        .await
        .expect("playbooks.run");
    let run_id = started["run_id"].as_str().expect("run id").to_string();
    common::finished(&lab.app, &started).await;
    let results = results(&lab.app, &run_id).await;
    assert_eq!(state_of(&results, "SW1"), "ok");
    let steps = steps_of(&results[0]);
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].1, "ok");
    assert!(steps[0].2.contains("(ignored)"), "{steps:?}");
}

#[tokio::test]
async fn the_template_steps_of_a_run_become_one_change_with_a_target_per_device() {
    let lab = lab(&["SW1", "SW2"]).await;
    lab.app
        .rpc(
            "templates.save",
            json!({"id": "ntp", "name": "NTP", "kind": "config",
                   "body": "ntp server {{ server }}",
                   "variables": [{"name": "server", "kind": "ip", "required": true}]}),
        )
        .await
        .expect("templates.save");
    lab.app
        .rpc(
            "templates.save",
            json!({"id": "dns", "name": "DNS", "kind": "config",
                   "body": "ip name-server 9.9.9.9\n! {{ device.name }}"}),
        )
        .await
        .expect("templates.save");
    let yaml = r#"
name: Baseline
steps:
  - name: NTP
    template:
      id: ntp
      vars: { server: 192.0.2.1 }
  - name: DNS
    template:
      id: dns
"#;
    let started = lab
        .app
        .rpc("playbooks.run", json!({"yaml": yaml, "filter": {}}))
        .await
        .expect("playbooks.run");
    let run_id = started["run_id"].as_str().expect("run id").to_string();
    common::finished(&lab.app, &started).await;

    let created = {
        let state = lab.app.state::<Changes>();
        let created = state.created.lock().expect("lock");
        created.clone()
    };
    assert_eq!(created.len(), 1, "one change for the whole run");
    assert_eq!(created[0]["origin"], json!("playbook"));
    let targets = created[0]["targets"].as_array().expect("targets");
    assert_eq!(targets.len(), 2);
    let lines_of = |name: &str| -> Value {
        let id = lab.id(name);
        targets
            .iter()
            .find(|target| target["device_id"] == json!(id))
            .expect("a target per device")["intent"]["lines"]
            .clone()
    };
    assert_eq!(
        lines_of("SW1"),
        json!(["ntp server 192.0.2.1", "ip name-server 9.9.9.9", "! SW1"]),
        "both template steps accumulated, in order"
    );
    assert_eq!(
        lines_of("SW2"),
        json!(["ntp server 192.0.2.1", "ip name-server 9.9.9.9", "! SW2"])
    );

    // The run links to the change, and nothing was sent.
    let run = lab
        .app
        .rpc("playbooks.run_get", json!({"run_id": run_id}))
        .await
        .expect("playbooks.run_get");
    assert_eq!(run["run"]["change_ids"], json!(["chg-1", "chg-1"]));
    assert_eq!(run["results"][0]["change_id"], json!("chg-1"));
    for name in ["SW1", "SW2"] {
        assert!(
            !lab.fake(name).running_config().contains("ntp server"),
            "{name} was configured directly"
        );
    }
}

#[tokio::test]
async fn a_device_that_failed_half_way_keeps_its_lines_out_of_the_change() {
    let lab = lab(&["SW1", "SW2"]).await;
    lab.app
        .rpc(
            "templates.save",
            json!({"id": "ntp", "name": "NTP", "kind": "config",
                   "body": "ntp server 192.0.2.1"}),
        )
        .await
        .expect("templates.save");
    let yaml = r#"
name: Half way
steps:
  - name: NTP
    template:
      id: ntp
  - name: Only SW1 may pass
    assert:
      that: "device.name == 'SW1'"
"#;
    let started = lab
        .app
        .rpc("playbooks.run", json!({"yaml": yaml, "filter": {}}))
        .await
        .expect("playbooks.run");
    let run_id = started["run_id"].as_str().expect("run id").to_string();
    common::finished(&lab.app, &started).await;

    let created = {
        let state = lab.app.state::<Changes>();
        let created = state.created.lock().expect("lock");
        created.clone()
    };
    let targets = created[0]["targets"].as_array().expect("targets");
    assert_eq!(
        targets.len(),
        1,
        "only the device that finished: {targets:?}"
    );
    assert_eq!(targets[0]["device_id"], json!(lab.id("SW1")));

    let results = results(&lab.app, &run_id).await;
    assert_eq!(state_of(&results, "SW2"), "failed");
    let sw2 = results
        .iter()
        .find(|result| result["name"] == json!("SW2"))
        .expect("SW2");
    assert_eq!(sw2.get("change_id"), None);
}

#[tokio::test]
async fn a_playbook_dry_run_opens_no_session_and_creates_no_change() {
    let lab = lab(&["SW1"]).await;
    lab.app
        .rpc(
            "templates.save",
            json!({"id": "ntp", "name": "NTP", "kind": "config",
                   "body": "ntp server 192.0.2.1"}),
        )
        .await
        .expect("templates.save");
    let yaml = r#"
name: Dry
steps:
  - name: Collect
    collect: [facts]
  - name: Read
    commands: [show version]
  - name: Back up
    backup: true
  - name: NTP
    template:
      id: ntp
"#;
    let started = lab
        .app
        .rpc(
            "playbooks.run",
            json!({"yaml": yaml, "device_ids": [lab.id("SW1")], "dry_run": true}),
        )
        .await
        .expect("playbooks.run");
    let run_id = started["run_id"].as_str().expect("run id").to_string();
    common::finished(&lab.app, &started).await;

    let results = results(&lab.app, &run_id).await;
    assert_eq!(state_of(&results, "SW1"), "ok");
    let steps = steps_of(&results[0]);
    assert!(steps[0].2.starts_with("would collect"), "{steps:?}");
    assert!(steps[1].2.starts_with("would run"), "{steps:?}");
    assert!(steps[2].2.starts_with("would back up"), "{steps:?}");
    assert!(
        steps[3].2.contains("configuration line"),
        "a template step still renders, so the lines can be seen: {steps:?}"
    );
    assert!(lab.fake("SW1").seen().is_empty(), "nothing was sent");
    assert!(
        lab.app
            .state::<Changes>()
            .created
            .lock()
            .expect("lock")
            .is_empty()
    );
    assert!(
        lab.app
            .state::<Backups>()
            .0
            .lock()
            .expect("lock")
            .is_empty()
    );
}

#[tokio::test]
async fn a_device_that_cannot_be_reached_is_unreached_not_failed() {
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
            "playbooks.run",
            json!({"yaml": "name: Read\nsteps:\n  - name: Read\n    commands: [show version]\n",
                   "filter": {}}),
        )
        .await
        .expect("playbooks.run");
    let run_id = started["run_id"].as_str().expect("run id").to_string();
    common::finished(&lab.app, &started).await;
    let results = results(&lab.app, &run_id).await;
    assert_eq!(state_of(&results, "SW1"), "ok");
    assert_eq!(state_of(&results, "GHOST"), "unreached");
}

#[tokio::test]
async fn a_saved_playbook_runs_by_id_and_is_listed() {
    let lab = lab(&["SW1"]).await;
    let saved = lab
        .app
        .rpc(
            "playbooks.save",
            json!({"name": "Health", "description": "before a change",
                   "yaml": "name: Health\nsteps:\n  - name: Read\n    commands: [show version]\n"}),
        )
        .await
        .expect("playbooks.save");
    let id = saved["id"].as_str().expect("an id").to_string();
    assert_eq!(
        lab.app
            .rpc("playbooks.list", json!({}))
            .await
            .expect("list")
            .as_array()
            .expect("array")
            .len(),
        1
    );
    let started = lab
        .app
        .rpc(
            "playbooks.run",
            json!({"playbook_id": id, "device_ids": [lab.id("SW1")]}),
        )
        .await
        .expect("playbooks.run");
    let run_id = started["run_id"].as_str().expect("run id").to_string();
    common::finished(&lab.app, &started).await;
    let run = lab
        .app
        .rpc("playbooks.run_get", json!({"run_id": run_id}))
        .await
        .expect("playbooks.run_get")["run"]
        .clone();
    assert_eq!(run["playbook_id"], json!(id));
    assert_eq!(run["name"], json!("Health"));
    assert_eq!(run["kind"], json!("playbook"));
    // `playbooks.runs` shows playbook runs only.
    assert_eq!(
        lab.app
            .rpc("playbooks.runs", json!({}))
            .await
            .expect("runs")
            .as_array()
            .expect("array")
            .len(),
        1
    );

    assert_eq!(
        lab.app
            .rpc("playbooks.delete", json!({"id": id}))
            .await
            .expect("deleted")["deleted"],
        json!(true)
    );
}

/// The rule this crate exists to keep: a second run never opens a second
/// session on a device the first run is still working on.
#[tokio::test(start_paused = true)]
async fn a_second_run_does_not_touch_a_device_the_first_is_still_working_on() {
    let (lab, _clock) = lab_at(&["SW1"], 1_700_000_000_000).await;
    let state = lab.app.state::<AutomationState>();
    let slow = "name: Slow\nsteps:\n  - name: Wait\n    pause: { seconds: 30 }\n  - name: Read\n    commands: [show version]\n";

    let first = lab
        .app
        .rpc(
            "playbooks.run",
            json!({"yaml": slow, "device_ids": [lab.id("SW1")], "name": "first"}),
        )
        .await
        .expect("the first run starts");
    // Wait until the first run really holds the device.
    let held = state.clone();
    let id = lab.id("SW1");
    until(move || held.holder(&id).is_some()).await;

    let second = lab
        .app
        .rpc(
            "playbooks.run",
            json!({"yaml": "name: Quick\nsteps:\n  - name: Read\n    commands: [show version]\n",
                   "device_ids": [lab.id("SW1")]}),
        )
        .await
        .expect("the second run starts");
    let second_run = second["run_id"].as_str().expect("run id").to_string();
    common::finished(&lab.app, &second).await;

    let results = results(&lab.app, &second_run).await;
    assert_eq!(results.len(), 1, "the device is still in the second run");
    assert_eq!(
        state_of(&results, "SW1"),
        "skipped",
        "not a second session: {results:?}"
    );
    assert!(
        results[0]["reason"]
            .as_str()
            .expect("a reason")
            .contains("Slow"),
        "the reason names the run that has it: {results:?}"
    );
    assert!(
        lab.fake("SW1").seen().is_empty(),
        "the first run is still waiting, so nothing has been sent at all yet"
    );

    // Let the first run finish; it does its own work undisturbed.
    tokio::time::advance(std::time::Duration::from_secs(31)).await;
    let job = common::finished(&lab.app, &first).await;
    assert_eq!(job.state, nocturne_app::jobs::JobState::Succeeded);
    assert!(
        lab.fake("SW1")
            .seen()
            .iter()
            .any(|line| line == "show version"),
        "and then it read the device"
    );
    assert!(
        state.holder(&lab.id("SW1")).is_none(),
        "the lease is released when the run ends"
    );
}

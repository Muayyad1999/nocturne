//! Ansible interoperability: what it promises, what it refuses, and what it
//! says it cannot guarantee.
//!
//! The process tests run against a script standing in for `ansible-playbook`,
//! which is what makes "credentials only through the environment", "the
//! temporary inventory is removed", "output is streamed" and "cancelling kills
//! it" testable at all. The last test runs the real binary when this machine
//! has one and says why it did not when it has not.

mod common;

use common::lab;
use serde_json::{Value, json};

/// A device Ansible could actually be pointed at: not a lab node.
async fn real_device(lab: &nocturne_app::testing::Lab, name: &str, credential: &str) -> String {
    lab.app
        .rpc(
            "devices.create",
            json!({"name": name, "address": "127.0.0.1", "profile": "cisco_ios",
                   "site": "DC", "role": "access", "credential_id": credential}),
        )
        .await
        .expect("devices.create")["id"]
        .as_str()
        .expect("an id")
        .to_string()
}

async fn credential(lab: &nocturne_app::testing::Lab, password: &str) -> String {
    lab.app
        .rpc(
            "credentials.create",
            json!({"name": "ansible", "username": "netops", "password": password,
                   "enable_secret": "en4ble-s3cret"}),
        )
        .await
        .expect("credentials.create")["id"]
        .as_str()
        .expect("an id")
        .to_string()
}

/// A login with no enable secret, so the inventory asks for no `become`.
async fn plain_credential(lab: &nocturne_app::testing::Lab, password: &str) -> String {
    lab.app
        .rpc(
            "credentials.create",
            json!({"name": "ansible-plain", "username": "netops", "password": password}),
        )
        .await
        .expect("credentials.create")["id"]
        .as_str()
        .expect("an id")
        .to_string()
}

#[tokio::test]
async fn status_says_what_this_machine_has_and_always_says_what_is_not_guaranteed() {
    let lab = lab(&["SW1"]).await;
    let status = lab
        .app
        .rpc("ansible.status", json!({}))
        .await
        .expect("ansible.status");
    let caveats = status["caveats"].as_array().expect("caveats");
    assert!(!caveats.is_empty(), "the caveats are always there");
    let first = caveats[0].as_str().expect("the first caveat");
    for wanted in ["backup", "drift", "rolled back", "change pipeline"] {
        assert!(
            first.contains(wanted),
            "the caveat must mention {wanted}: {first}"
        );
    }
    if status["installed"] == json!(true) {
        assert!(
            status["version"].as_str().is_some_and(|v| !v.is_empty()),
            "an installed Ansible says its version: {status:?}"
        );
        assert!(status["path"].as_str().is_some());
        assert!(status["unsupported"].is_null());
    } else {
        let reason = status["unsupported"].as_str().expect("a reason");
        assert!(
            reason.contains("ansible-playbook") && reason.contains("Nothing else in Nocturne"),
            "not installed is a clean unsupported with a reason: {reason}"
        );
    }

    // A machine without Ansible refuses a run as `unsupported`, not as a failure.
    lab.app
        .rpc(
            "settings.set",
            json!({"section": "automation",
                   "value": {"ansible_path": "/nonexistent/ansible-playbook"}}),
        )
        .await
        .expect("a path that is not there");
    let status = lab
        .app
        .rpc("ansible.status", json!({}))
        .await
        .expect("ansible.status");
    assert_eq!(status["installed"], json!(false));
    assert!(status["unsupported"].as_str().is_some());
}

#[tokio::test]
async fn a_change_exports_as_a_playbook_and_an_inventory_with_no_secret_in_either() {
    let lab = lab(&["SW1"]).await;
    let credential = credential(&lab, "s3cret-ansible-pw").await;
    let device = real_device(&lab, "SW-A", &credential).await;
    let created = lab
        .app
        .rpc(
            "changes.create",
            json!({"title": "NTP", "targets": [{
                "device_id": device,
                "intent": {"type": "lines", "lines": ["ntp server 192.0.2.1", "ntp source Vlan1"]}
            }]}),
        )
        .await
        .expect("the stub change");

    let exported = lab
        .app
        .rpc(
            "ansible.export_playbook",
            json!({"change_id": created["change_id"]}),
        )
        .await
        .expect("ansible.export_playbook");
    let playbook = exported["playbook"].as_str().expect("a playbook");
    let inventory = exported["inventory"].as_str().expect("an inventory");

    // Both are YAML, and say what they are.
    let parsed: Value = serde_yaml_ng::from_str(playbook).expect("the playbook is YAML");
    let plays = parsed.as_array().expect("a list of plays");
    assert_eq!(plays.len(), 1);
    assert_eq!(plays[0]["hosts"], json!("SW-A"));
    assert_eq!(
        plays[0]["tasks"][0]["cisco.ios.ios_config"]["lines"],
        json!(["ntp server 192.0.2.1", "ntp source Vlan1"]),
        "the lines go to the platform's own config module"
    );
    let parsed: Value = serde_yaml_ng::from_str(inventory).expect("the inventory is YAML");
    let host = &parsed["all"]["hosts"]["SW-A"];
    assert_eq!(host["ansible_host"], json!("127.0.0.1"));
    assert_eq!(host["ansible_network_os"], json!("cisco.ios.ios"));
    assert_eq!(host["ansible_user"], json!("netops"));
    assert_eq!(
        host["ansible_password"],
        json!("{{ lookup('env', 'NOCTURNE_PW_1') }}"),
        "the inventory names the environment variable, never the password"
    );
    assert_eq!(host["ansible_become"], json!(true));
    assert_eq!(
        parsed["all"]["children"]["site_dc"]["hosts"]["SW-A"],
        json!({}),
        "the groups a team expects are there"
    );

    for secret in ["s3cret-ansible-pw", "en4ble-s3cret"] {
        assert!(!playbook.contains(secret), "the playbook leaks {secret}");
        assert!(!inventory.contains(secret), "the inventory leaks {secret}");
    }
    assert!(
        playbook.contains("rolled back"),
        "the export carries the caveat: {playbook}"
    );
}

#[tokio::test]
async fn a_template_exports_as_a_playbook_too_and_an_unmappable_platform_is_unsupported() {
    let lab = lab(&["SW1"]).await;
    let credential = credential(&lab, "s3cret-ansible-pw").await;
    real_device(&lab, "SW-A", &credential).await;
    lab.app
        .rpc(
            "templates.save",
            json!({"id": "ntp", "name": "NTP", "kind": "config",
                   "body": "ntp server {{ server }}",
                   "variables": [{"name": "server", "kind": "ip", "required": true}]}),
        )
        .await
        .expect("templates.save");
    let exported = lab
        .app
        .rpc(
            "ansible.export_playbook",
            json!({"template_id": "ntp", "device_ids": ["SW-A"],
                   "vars": {"server": "192.0.2.1"}}),
        )
        .await
        .expect("ansible.export_playbook");
    let parsed: Value =
        serde_yaml_ng::from_str(exported["playbook"].as_str().expect("yaml")).expect("YAML");
    assert_eq!(
        parsed[0]["tasks"][0]["cisco.ios.ios_config"]["lines"],
        json!(["ntp server 192.0.2.1"])
    );

    // The lab device's type has no module Nocturne is sure of: say so, by name.
    let err = lab
        .app
        .rpc(
            "ansible.export_playbook",
            json!({"template_id": "ntp", "device_ids": [lab.id("SW1")],
                   "vars": {"server": "192.0.2.1"}}),
        )
        .await
        .expect_err("no mapping for test_ios");
    assert_eq!(err.code, "unsupported");
    assert!(
        err.message.contains("test_ios") && err.message.contains("cisco_ios"),
        "{}",
        err.message
    );
}

#[tokio::test]
async fn a_template_is_not_exported_for_a_device_it_is_not_written_for() {
    // The playbook would push RouterOS lines with the IOS module: the same
    // refusal `templates.apply` gives.
    let lab = lab(&["SW1"]).await;
    let credential = credential(&lab, "s3cret-ansible-pw").await;
    real_device(&lab, "SW-A", &credential).await;
    let err = lab
        .app
        .rpc(
            "ansible.export_playbook",
            json!({"template_id": "routeros_ntp_client", "device_ids": ["SW-A"],
                   "vars": {"servers": ["192.0.2.1"]}}),
        )
        .await
        .expect_err("RouterOS lines for an IOS device");
    assert_eq!(err.code, "invalid");
    assert!(
        err.message.contains("mikrotik_routeros") && err.message.contains("SW-A (cisco_ios)"),
        "{}",
        err.message
    );
}

#[cfg(unix)]
mod process {
    use super::*;
    use std::path::{Path, PathBuf};

    /// Write an executable stand-in for `ansible-playbook`.
    fn script(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("ansible-playbook");
        std::fs::write(&path, body).expect("the script is written");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("executable");
        path
    }

    async fn use_script(lab: &nocturne_app::testing::Lab, path: &Path) {
        lab.app
            .rpc(
                "settings.set",
                json!({"section": "automation", "value": {"ansible_path": path}}),
            )
            .await
            .expect("the stand-in is configured");
    }

    /// What the script saw: the arguments, the `NOCTURNE_*` environment, and a
    /// copy of the inventory it was handed.
    fn probe_body(probe: &Path, extra: &str) -> String {
        format!(
            r#"#!/bin/sh
probe='{}'
echo "$@" > "$probe/args"
env | grep '^NOCTURNE_' | sort > "$probe/env"
inventory=''
previous=''
for argument in "$@"; do
  if [ "$previous" = "-i" ]; then inventory="$argument"; fi
  previous="$argument"
done
echo "$inventory" > "$probe/inventory_path"
cp "$inventory" "$probe/inventory"
ls -l "$inventory" | cut -c1-10 > "$probe/mode"
{extra}
"#,
            probe.display()
        )
    }

    #[tokio::test]
    async fn credentials_reach_the_playbook_only_through_the_environment() {
        let lab = lab(&["SW1"]).await;
        let credential = credential(&lab, "s3cret-ansible-pw").await;
        real_device(&lab, "SW-A", &credential).await;
        real_device(&lab, "SW-B", &credential).await;

        let home = tempfile::tempdir().expect("a temporary directory");
        let probe = home.path().join("probe");
        std::fs::create_dir_all(&probe).expect("the probe directory");
        let playbook = home.path().join("site.yml");
        std::fs::write(&playbook, "- hosts: all\n  tasks: []\n").expect("a playbook file");
        let script = script(
            home.path(),
            &probe_body(
                &probe,
                r#"echo "the password is $NOCTURNE_PW_1"
echo "a warning" 1>&2
echo ''
echo 'PLAY RECAP *********'
echo 'SW-A                       : ok=2    changed=1    unreachable=0    failed=0    skipped=0'
echo 'SW-B                       : ok=0    changed=0    unreachable=1    failed=0    skipped=0'
"#,
            ),
        );
        use_script(&lab, &script).await;

        let started = lab
            .app
            .rpc(
                "ansible.run",
                json!({"playbook_path": playbook, "filter": {"site": "DC"}}),
            )
            .await
            .expect("ansible.run");
        let job = common::finished(&lab.app, &started).await;
        assert_eq!(
            job.state,
            nocturne_app::jobs::JobState::Succeeded,
            "{job:?}"
        );

        // The credentials were in the environment, and only there.
        let environment = std::fs::read_to_string(probe.join("env")).expect("the env probe");
        assert!(
            environment.contains("NOCTURNE_PW_1=s3cret-ansible-pw"),
            "the password reached the process: {environment}"
        );
        assert!(
            environment.contains("NOCTURNE_EN_1=en4ble-s3cret"),
            "and so did the enable secret: {environment}"
        );
        let arguments = std::fs::read_to_string(probe.join("args")).expect("the args probe");
        for secret in ["s3cret-ansible-pw", "en4ble-s3cret"] {
            assert!(
                !arguments.contains(secret),
                "a secret was on the command line: {arguments}"
            );
        }
        let inventory = std::fs::read_to_string(probe.join("inventory")).expect("the inventory");
        for secret in ["s3cret-ansible-pw", "en4ble-s3cret"] {
            assert!(
                !inventory.contains(secret),
                "a secret was written to a file: {inventory}"
            );
        }
        // YAML doubles the single quotes when it emits the scalar, so the
        // check is on the substance: the variable's name, and a lookup.
        assert!(
            inventory.contains("NOCTURNE_PW_1") && inventory.contains("lookup("),
            "the inventory refers to the environment: {inventory}"
        );
        assert_eq!(
            std::fs::read_to_string(probe.join("mode"))
                .expect("the mode probe")
                .trim(),
            "-rw-------",
            "the temporary inventory is readable by nobody else"
        );
        assert!(
            arguments.contains("--check"),
            "check mode by default: {arguments}"
        );

        // The temporary inventory is gone, and so is the directory it was in.
        let path = std::fs::read_to_string(probe.join("inventory_path")).expect("the path");
        let path = PathBuf::from(path.trim());
        assert!(!path.exists(), "{} was left behind", path.display());
        assert!(!path.parent().expect("a parent").exists());

        // The process's output was streamed into the job, with the secret
        // scrubbed out of it.
        let log: String = job
            .log
            .iter()
            .map(|line| line.message.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            log.contains("the password is ********"),
            "a secret the playbook printed is scrubbed: {log}"
        );
        assert!(!log.contains("s3cret-ansible-pw"), "{log}");
        assert!(log.contains("a warning"), "stderr is streamed too: {log}");
        assert!(
            job.log
                .iter()
                .any(|line| line.level == "warn" && line.message.contains("rolled back")),
            "every run says what it does not guarantee: {log}"
        );

        // And the per-host recap became per-device results.
        let run_id = job.result.as_ref().expect("a result")["run_id"]
            .as_str()
            .expect("a run id")
            .to_string();
        let results = common::results(&lab.app, &run_id).await;
        assert_eq!(common::state_of(&results, "SW-A"), "ok");
        assert_eq!(
            common::state_of(&results, "SW-B"),
            "unreached",
            "a host Ansible could not reach is unreached, like any other"
        );
        let run = lab
            .app
            .rpc("runner.get", json!({"run_id": run_id}))
            .await
            .expect("runner.get")["run"]
            .clone();
        assert_eq!(run["kind"], json!("ansible"));
        assert!(
            run["caveat"]
                .as_str()
                .expect("the caveat is on the record")
                .contains("outside Nocturne's change pipeline")
        );
    }

    #[tokio::test]
    async fn a_host_the_playbook_did_not_touch_is_skipped_with_the_reason() {
        let lab = lab(&["SW1"]).await;
        let credential = credential(&lab, "s3cret-ansible-pw").await;
        real_device(&lab, "SW-A", &credential).await;
        real_device(&lab, "SW-B", &credential).await;
        let home = tempfile::tempdir().expect("a temporary directory");
        let probe = home.path().join("probe");
        std::fs::create_dir_all(&probe).expect("the probe directory");
        let playbook = home.path().join("site.yml");
        std::fs::write(&playbook, "- hosts: SW-A\n  tasks: []\n").expect("a playbook");
        let script = script(
            home.path(),
            &probe_body(
                &probe,
                "echo 'PLAY RECAP ***'\necho 'SW-A : ok=1 changed=0 unreachable=0 failed=0 skipped=0'\n",
            ),
        );
        use_script(&lab, &script).await;
        let started = lab
            .app
            .rpc(
                "ansible.run",
                json!({"playbook_path": playbook, "filter": {"site": "DC"}}),
            )
            .await
            .expect("ansible.run");
        let job = common::finished(&lab.app, &started).await;
        let run_id = job.result.as_ref().expect("a result")["run_id"]
            .as_str()
            .expect("a run id")
            .to_string();
        let results = common::results(&lab.app, &run_id).await;
        assert_eq!(common::state_of(&results, "SW-A"), "ok");
        assert_eq!(common::state_of(&results, "SW-B"), "skipped");
        let skipped = results
            .iter()
            .find(|result| result["name"] == json!("SW-B"))
            .expect("SW-B");
        assert!(
            skipped["reason"]
                .as_str()
                .expect("a reason")
                .contains("PLAY RECAP"),
            "{skipped:?}"
        );
    }

    #[tokio::test]
    async fn cancelling_the_job_kills_the_process_and_cleans_up() {
        let lab = lab(&["SW1"]).await;
        let credential = credential(&lab, "s3cret-ansible-pw").await;
        real_device(&lab, "SW-A", &credential).await;
        let home = tempfile::tempdir().expect("a temporary directory");
        let probe = home.path().join("probe");
        std::fs::create_dir_all(&probe).expect("the probe directory");
        let playbook = home.path().join("site.yml");
        std::fs::write(&playbook, "- hosts: all\n  tasks: []\n").expect("a playbook");
        // `exec` so that killing the child kills the sleep, not just a shell.
        let script = script(
            home.path(),
            &probe_body(&probe, "echo running\nexec sleep 120\n"),
        );
        use_script(&lab, &script).await;

        let started = lab
            .app
            .rpc(
                "ansible.run",
                json!({"playbook_path": playbook, "device_ids": ["SW-A"], "check": false}),
            )
            .await
            .expect("ansible.run");
        let job_id = started["job_id"].as_str().expect("a job id").to_string();
        // Wait until the process has really started.
        for _ in 0..200 {
            if probe.join("inventory_path").exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(probe.join("inventory_path").exists(), "the script ran");

        lab.app
            .rpc("jobs.cancel", json!({"id": job_id}))
            .await
            .expect("jobs.cancel");
        let job = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            common::finished(&lab.app, &started),
        )
        .await
        .expect("cancelling ends the job rather than waiting for the sleep");
        assert_eq!(job.state, nocturne_app::jobs::JobState::Cancelled);

        let path = std::fs::read_to_string(probe.join("inventory_path")).expect("the path");
        let path = PathBuf::from(path.trim());
        assert!(
            !path.exists(),
            "the temporary inventory survived a cancel: {}",
            path.display()
        );
        let arguments = std::fs::read_to_string(probe.join("args")).expect("the args");
        assert!(
            !arguments.contains("--check"),
            "`check: false` is honoured: {arguments}"
        );
    }

    #[tokio::test]
    async fn a_simulated_device_is_skipped_because_ansible_has_nothing_to_connect_to() {
        let lab = lab(&["SW1"]).await;
        let home = tempfile::tempdir().expect("a temporary directory");
        let playbook = home.path().join("site.yml");
        std::fs::write(&playbook, "- hosts: all\n  tasks: []\n").expect("a playbook");
        let script = script(home.path(), "#!/bin/sh\necho nothing\n");
        use_script(&lab, &script).await;
        let started = lab
            .app
            .rpc(
                "ansible.run",
                json!({"playbook_path": playbook, "device_ids": [lab.id("SW1")]}),
            )
            .await
            .expect("ansible.run");
        let job = common::finished(&lab.app, &started).await;
        let run_id = job.result.as_ref().expect("a result")["run_id"]
            .as_str()
            .expect("a run id")
            .to_string();
        let results = common::results(&lab.app, &run_id).await;
        assert_eq!(common::state_of(&results, "SW1"), "skipped");
        assert!(
            results[0]["reason"]
                .as_str()
                .expect("a reason")
                .contains("simulated device"),
            "{results:?}"
        );
    }

    #[tokio::test]
    async fn arguments_and_paths_that_would_break_the_promises_are_refused() {
        let lab = lab(&["SW1"]).await;
        let credential = credential(&lab, "s3cret-ansible-pw").await;
        real_device(&lab, "SW-A", &credential).await;
        let home = tempfile::tempdir().expect("a temporary directory");
        let playbook = home.path().join("site.yml");
        std::fs::write(&playbook, "- hosts: all\n  tasks: []\n").expect("a playbook");
        let script = script(home.path(), "#!/bin/sh\necho nothing\n");
        use_script(&lab, &script).await;

        for argument in ["-i", "--inventory=hosts.yml", "--ask-pass", "--limit"] {
            let err = lab
                .app
                .rpc(
                    "ansible.run",
                    json!({"playbook_path": playbook, "device_ids": ["SW-A"],
                           "extra_args": [argument]}),
                )
                .await
                .expect_err("refused");
            assert_eq!(err.code, "invalid", "{argument}");
            assert!(err.message.contains(argument), "{}", err.message);
        }
        let err = lab
            .app
            .rpc(
                "ansible.run",
                json!({"playbook_path": home.path().join("nope.yml"), "device_ids": ["SW-A"]}),
            )
            .await
            .expect_err("no such playbook");
        assert_eq!(err.code, "not_found");
    }

    /// The real thing, when this machine has it.
    #[tokio::test]
    async fn a_real_ansible_playbook_runs_and_reports_per_host() {
        let on_path = std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join("ansible-playbook").is_file())
        });
        if !on_path {
            eprintln!(
                "skipped: `ansible-playbook` is not on PATH, so the real binary cannot be \
                 exercised here. Install Ansible (`pipx install ansible-core`) and run again."
            );
            return;
        }

        let lab = lab(&["SW1"]).await;
        // No enable secret either: `ansible_become_method: enable` comes from
        // the `ansible.netcommon` collection, which plain `ansible-core` does
        // not have. That is a real constraint of the Ansible path, not of this
        // test, and it is in the caveats `ansible.status` returns.
        let credential = plain_credential(&lab, "s3cret-ansible-pw").await;
        // Devices with no device type: Nocturne then puts no
        // `ansible_network_os`/`ansible_connection` in the inventory, so the
        // play's own `connection: local` stands and the test needs neither an
        // SSH server nor the `ansible.netcommon` collection. Everything else —
        // the generated inventory, the credentials in the environment, the
        // streaming, the recap — is the real path.
        for name in ["SW-A", "SW-B"] {
            lab.app
                .rpc(
                    "devices.create",
                    json!({"name": name, "address": "127.0.0.1", "site": "Local",
                           "credential_id": credential}),
                )
                .await
                .expect("devices.create");
        }
        let home = tempfile::tempdir().expect("a temporary directory");
        let playbook = home.path().join("site.yml");
        std::fs::write(
            &playbook,
            r#"- name: What Nocturne handed us
  hosts: all
  gather_facts: false
  connection: local
  tasks:
    - name: The password came from the environment
      ansible.builtin.assert:
        that:
          - ansible_password | length > 0
          - ansible_user == 'netops'
        quiet: true
    - name: Say which host this is
      ansible.builtin.debug:
        msg: "nocturne ran me on {{ inventory_hostname }}"
"#,
        )
        .expect("a playbook file");

        let started = lab
            .app
            .rpc(
                "ansible.run",
                json!({"playbook_path": playbook, "filter": {"site": "Local"}}),
            )
            .await
            .expect("ansible.run");
        let job = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            common::finished(&lab.app, &started),
        )
        .await
        .expect("ansible finished");
        let log: String = job
            .log
            .iter()
            .map(|line| line.message.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            job.state,
            nocturne_app::jobs::JobState::Succeeded,
            "{:?}\n{log}",
            job.error
        );
        assert!(
            log.contains("nocturne ran me on SW-A") && log.contains("nocturne ran me on SW-B"),
            "the real output was streamed into the job: {log}"
        );
        assert!(
            !log.contains("s3cret-ansible-pw"),
            "the password must not appear in the job log: {log}"
        );

        let run_id = job.result.as_ref().expect("a result")["run_id"]
            .as_str()
            .expect("a run id")
            .to_string();
        let results = common::results(&lab.app, &run_id).await;
        assert_eq!(results.len(), 2);
        for name in ["SW-A", "SW-B"] {
            assert_eq!(
                common::state_of(&results, name),
                "ok",
                "{name}: {results:?}\n{log}"
            );
        }
        let steps = results[0]["steps"].as_array().expect("steps");
        assert!(
            steps[0]["detail"]
                .as_str()
                .expect("a detail")
                .starts_with("ok=2"),
            "the recap was parsed: {steps:?}"
        );
    }
}

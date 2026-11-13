//! Planning: the commands, the prediction, the rollback, the risk and the
//! protected resources — all without touching the device.

mod common;

use common::*;
use serde_json::json;

#[tokio::test]
async fn lines_are_sent_as_typed_and_predicted_exactly() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let change = planned(
        app,
        &lab.id("SW1"),
        json!({"type": "lines", "lines": [
            "vlan 30",
            "name voice",
            "exit",
            "interface GigabitEthernet0/2",
            "description desk 14",
            "exit",
        ]}),
        json!({}),
    )
    .await;

    assert_eq!(
        commands(&change),
        vec![
            "vlan 30",
            "name voice",
            "exit",
            "interface GigabitEthernet0/2",
            "description desk 14",
            "exit"
        ]
    );
    // The rollback takes the new VLAN away and puts the old description back.
    assert_eq!(
        rollback(&change),
        vec![
            "interface GigabitEthernet0/2",
            "description desk 12",
            "exit",
            "no vlan 30",
        ]
    );
    let plan = plan(&change);
    assert_eq!(plan["risk"]["level"], "low", "{:#?}", plan["risk"]);
    assert!(plan["protected_hits"].as_array().unwrap().is_empty());
    assert_eq!(plan["profile"], VERIFIED_PROFILE);
    assert_eq!(plan["maturity"], "verified");
    assert!(plan["base_version_id"].as_i64().unwrap() > 0);
    assert_eq!(plan["base_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(plan["plan_hash"].as_str().unwrap().len(), 64);
    // The predicted difference is the four new lines.
    let added: Vec<String> = plan["diff"]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|line| line["tag"] == "+")
        .map(|line| line["text"].as_str().unwrap().trim().to_string())
        .collect();
    assert_eq!(
        added,
        vec!["description desk 14", "vlan 30", "name voice"],
        "{:#?}",
        plan["diff"]
    );
    assert_eq!(plan["diff"]["removed"], 1, "the old description goes");
    // Nothing was sent to the device.
    assert!(
        !lab.fake("SW1").running_config().contains("vlan 30"),
        "planning must not change anything"
    );
    assert_eq!(change["state"], "planned");
}

#[tokio::test]
async fn an_intended_configuration_is_turned_into_a_difference_both_ways() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let running = lab.fake("SW1").running_config();
    let intended = running
        .replace("description desk 12", "description desk 14")
        .replace(
            "vlan 20\n name voice\n",
            "vlan 20\n name voice\n!\nvlan 30\n name guests\n",
        );

    // Exact: what is not in the intended configuration is removed.
    let exact = planned(
        app,
        &lab.id("SW1"),
        json!({"type": "intended", "config": intended, "exact": true}),
        json!({}),
    )
    .await;
    assert_eq!(
        commands(&exact),
        vec![
            "vlan 30",
            "name guests",
            "exit",
            "interface GigabitEthernet0/2",
            "description desk 14",
            "exit",
        ]
    );

    // Additive: the same configuration minus a line is a no-op, not a removal.
    let shorter = running.replace(" description desk 12\n", "");
    let additive = planned(
        app,
        &lab.id("SW1"),
        json!({"type": "intended", "config": shorter, "exact": false}),
        json!({}),
    )
    .await;
    assert!(commands(&additive).is_empty(), "{:?}", commands(&additive));
    assert!(
        plan(&additive)["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("already matches"))
    );

    let exact_removal = planned(
        app,
        &lab.id("SW1"),
        json!({"type": "intended", "config": shorter, "exact": true}),
        json!({}),
    )
    .await;
    assert_eq!(
        commands(&exact_removal),
        vec!["interface GigabitEthernet0/2", "no description", "exit"]
    );
}

#[tokio::test]
async fn a_trunk_delta_uses_add_and_remove_but_a_typed_list_is_a_protected_hit() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    // Give the uplink a VLAN list to start from.
    load(
        &lab,
        "SW1",
        &BASE_CONFIG.replace(
            " switchport mode trunk\n",
            " switchport mode trunk\n switchport trunk allowed vlan 10,20\n",
        ),
    );

    let running = lab.fake("SW1").running_config();
    let intended = running.replace("allowed vlan 10,20", "allowed vlan 10,30");
    let delta = planned(
        app,
        &lab.id("SW1"),
        json!({"type": "intended", "config": intended, "exact": true}),
        json!({}),
    )
    .await;
    assert_eq!(
        commands(&delta),
        vec![
            "interface GigabitEthernet0/1",
            "switchport trunk allowed vlan add 30",
            "switchport trunk allowed vlan remove 20",
            "exit",
        ],
        "a delta is add/remove, never a replacing list"
    );

    // The same thing typed by hand replaces the list: that is the hit.
    let typed = planned(
        app,
        &lab.id("SW1"),
        json!({"type": "lines", "lines": [
            "interface GigabitEthernet0/1",
            "switchport trunk allowed vlan 10,30",
            "exit",
        ]}),
        json!({}),
    )
    .await;
    assert!(
        hit_kinds(&typed).contains(&"trunk_allowed_replace".to_string()),
        "{:#?}",
        plan(&typed)["protected_hits"]
    );
    let why = plan(&typed)["protected_hits"][0]["why"].as_str().unwrap();
    assert!(why.contains("20"), "the lost VLAN is named: {why}");
    assert!(why.contains("add"), "the safe form is suggested: {why}");
    assert_eq!(plan(&typed)["risk"]["level"], "high");
}

#[tokio::test]
async fn the_management_path_and_the_way_in_are_protected() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    // The fake answers on 10.0.0.2, which is on Vlan1.
    app.rpc(
        "devices.update",
        json!({"id": lab.id("SW1"), "address": "10.0.0.2"}),
    )
    .await
    .unwrap();

    let cases: Vec<(Vec<&str>, &str)> = vec![
        (
            vec!["interface Vlan1", "shutdown", "exit"],
            "management_interface",
        ),
        (
            vec![
                "interface Vlan1",
                "ip address 10.0.0.9 255.255.255.0",
                "exit",
            ],
            "management_interface",
        ),
        (vec!["no ip default-gateway 10.0.0.1"], "default_gateway"),
        (vec!["ip route 0.0.0.0 0.0.0.0 10.0.0.9"], "default_route"),
        (
            vec!["line vty 0 4", "transport input telnet", "exit"],
            "access_vty",
        ),
        (
            vec!["username bob privilege 15 secret cisco"],
            "access_user",
        ),
        (vec!["no vlan 10"], "vlan_removal"),
        (vec!["hostname SW1-NEW"], "hostname"),
        (vec!["spanning-tree mode mst"], "spanning_tree"),
    ];
    for (lines, expected) in cases {
        let change = planned(
            app,
            &lab.id("SW1"),
            json!({"type": "lines", "lines": lines}),
            json!({}),
        )
        .await;
        assert!(
            hit_kinds(&change).contains(&expected.to_string()),
            "{lines:?} should be a `{expected}` hit, got {:?}",
            hit_kinds(&change)
        );
        let level = plan(&change)["risk"]["level"].as_str().unwrap().to_string();
        assert!(level == "high" || level == "critical", "{lines:?}: {level}");
    }
}

#[tokio::test]
async fn protected_resources_are_found_however_the_line_is_dressed_up() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    app.rpc(
        "devices.update",
        json!({"id": lab.id("SW1"), "address": "10.0.0.2"}),
    )
    .await
    .unwrap();

    // Upper case and extra spaces are refused outright, before anything else
    // has a chance to be fooled by them.
    for lines in [
        vec!["INTERFACE Vlan1", "SHUTDOWN"],
        vec!["interface  Vlan1", "   shut   "],
    ] {
        let err = app
            .rpc(
                "changes.preview",
                json!({"device_id": lab.id("SW1"), "intent": {"type": "lines", "lines": lines}}),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, "invalid", "{lines:?}");
    }

    // The detector itself is blind to case and spacing, and follows the
    // section a line lands in however far away the header is.
    let hidden = planned(
        app,
        &lab.id("SW1"),
        json!({"type": "lines", "lines": [
            "interface Vlan1",
            "description management svi",
            "ip address 10.0.0.3 255.255.255.0",
            "exit",
        ]}),
        json!({}),
    )
    .await;
    assert!(hit_kinds(&hidden).contains(&"management_interface".to_string()));

    // `default interface` on an uplink with a neighbour.
    let defaulted = planned(
        app,
        &lab.id("SW1"),
        json!({"type": "lines", "lines": ["default interface GigabitEthernet0/1"]}),
        json!({}),
    )
    .await;
    assert!(
        hit_kinds(&defaulted).contains(&"uplink".to_string()),
        "{:?}",
        hit_kinds(&defaulted)
    );
    assert!(
        plan(&defaulted)["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("resets the port"))
    );

    // `interface range` cannot be predicted, so it is refused.
    let err = app
        .rpc(
            "changes.preview",
            json!({"device_id": lab.id("SW1"), "intent": {"type": "lines", "lines": [
                "interface range GigabitEthernet0/1 - 2", "shutdown"
            ]}}),
        )
        .await
        .unwrap_err();
    assert!(err.message.contains("several ports"), "{}", err.message);

    // A protected device needs a confirmation whatever the change is.
    app.rpc(
        "devices.update",
        json!({"id": lab.id("SW1"), "protected": true}),
    )
    .await
    .unwrap();
    let trivial = planned(
        app,
        &lab.id("SW1"),
        json!({"type": "lines", "lines": ["interface GigabitEthernet0/2", "description x", "exit"]}),
        json!({}),
    )
    .await;
    assert_eq!(hit_kinds(&trivial), vec!["protected_device"]);
}

#[tokio::test]
async fn shutting_down_a_port_with_a_neighbour_is_protected() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    // Collect the device's neighbours first: that is where the knowledge comes from.
    run_job(
        app,
        "collect.run",
        json!({"device_ids": [lab.id("SW1")], "kinds": ["neighbors", "interfaces"]}),
    )
    .await;

    let change = planned(
        app,
        &lab.id("SW1"),
        json!({"type": "lines", "lines": ["interface GigabitEthernet0/1", "shutdown", "exit"]}),
        json!({}),
    )
    .await;
    let hits = plan(&change)["protected_hits"].as_array().unwrap().clone();
    assert!(
        hits.iter()
            .any(|hit| hit["kind"] == "uplink" && hit["why"].as_str().unwrap().contains("CORE1")),
        "{hits:#?}"
    );

    // A port with nothing on it is ordinary work.
    let quiet = planned(
        app,
        &lab.id("SW1"),
        json!({"type": "lines", "lines": ["interface GigabitEthernet0/2", "shutdown", "exit"]}),
        json!({}),
    )
    .await;
    assert!(hit_kinds(&quiet).is_empty(), "{:?}", hit_kinds(&quiet));
}

#[tokio::test]
async fn dangerous_commands_and_malformed_lines_are_refused_at_planning() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let preview = |lines: Vec<&str>| {
        let app = app.clone();
        let id = lab.id("SW1");
        let lines: Vec<String> = lines.into_iter().map(String::from).collect();
        async move {
            app.rpc(
                "changes.preview",
                json!({"device_id": id, "intent": {"type": "lines", "lines": lines}}),
            )
            .await
            .unwrap_err()
        }
    };

    // The pipeline configures; it does not operate.
    let err = preview(vec!["reload in 1"]).await;
    assert_eq!(err.code, "forbidden");
    assert!(err.message.contains("does not operate"), "{}", err.message);

    // A sub-command with no section header is an error, not a guess.
    let err = preview(vec!["switchport access vlan 30"]).await;
    assert!(err.message.contains("sub-command"), "{}", err.message);

    // Abbreviations are refused with the full form.
    let err = preview(vec!["int gi0/2", "desc x"]).await;
    assert!(err.message.contains("`interface`"), "{}", err.message);

    // A global command inside an open section has to say so.
    let err = preview(vec!["interface GigabitEthernet0/2", "hostname OTHER"]).await;
    assert!(err.message.contains("exit"), "{}", err.message);

    // Nothing of this reached the device.
    assert!(
        !lab.fake("SW1")
            .seen()
            .iter()
            .any(|command| command.contains("reload") || command.contains("hostname"))
    );
}

#[tokio::test]
async fn an_intended_configuration_is_refused_where_it_cannot_be_derived() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    // A profile whose grammar is not modelled.
    app.rpc(
        "profiles.save",
        json!({"yaml": "id: odd_box\nname: Odd box\nmaturity: community\ncli:\n  prompt: '^(?P<host>[\\w.\\-]{1,63})(\\([\\w\\-/. ]+\\))?[>#]\\s?$'\n  user_prompt: '^[\\w.\\-]{1,63}>\\s?$'\n  config_prompt: '^[\\w.\\-]{1,63}\\([\\w\\-/. ]+\\)#\\s?$'\n  enable: {}\n  init: ['terminal length 0']\n  errors: ['^% ']\nconfig:\n  syntax: flat\n  enter: ['configure terminal']\n  exit: ['end']\n  save: ['write memory']\n  running: 'show running-config'\n  errors: ['^% ']\npolicy:\n  read_only: ['^show ', '^ping ']\n  dangerous: ['^reload']\n"}),
    )
    .await
    .unwrap();
    app.rpc(
        "devices.update",
        json!({"id": lab.id("SW1"), "profile": "odd_box"}),
    )
    .await
    .unwrap();

    let err = app
        .rpc(
            "changes.preview",
            json!({"device_id": lab.id("SW1"), "intent": {"type": "intended", "config": "hostname X\n", "exact": true}}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "invalid");
    assert!(err.message.contains("cannot work out"), "{}", err.message);

    // Hand-written lines still work there — but nothing can be predicted, so
    // the change can never be low risk.
    let plan = app
        .rpc(
            "changes.preview",
            json!({"device_id": lab.id("SW1"), "intent": {"type": "lines", "lines": ["ntp server 192.0.2.1"]}}),
        )
        .await
        .unwrap();
    assert_eq!(plan["commands"], json!(["ntp server 192.0.2.1"]));
    assert_ne!(plan["risk"]["level"], "low");
    assert!(
        plan["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("cannot be predicted")),
        "{:#?}",
        plan["warnings"]
    );
}

#[tokio::test]
async fn a_protected_line_cannot_hide_behind_a_section_change_or_a_negation() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    app.rpc(
        "devices.update",
        json!({"id": lab.id("SW1"), "address": "10.0.0.2"}),
    )
    .await
    .unwrap();
    run_job(
        app,
        "collect.run",
        json!({"device_ids": [lab.id("SW1")], "kinds": ["neighbors"]}),
    )
    .await;

    // The section header is three lines above, with an innocent one in between.
    let hidden = planned(
        app,
        &lab.id("SW1"),
        json!({"type": "lines", "lines": [
            "interface GigabitEthernet0/2",
            "description a quiet port",
            "exit",
            "interface Vlan1",
            "description management",
            "shutdown",
            "exit",
        ]}),
        json!({}),
    )
    .await;
    assert!(
        hit_kinds(&hidden).contains(&"management_interface".to_string()),
        "{:#?}",
        plan(&hidden)["protected_hits"]
    );
    assert_eq!(plan(&hidden)["risk"]["level"], "critical");

    // Negations of the same things are hits too.
    for (lines, expected) in [
        (vec!["no interface Vlan1"], "management_interface"),
        (
            vec![
                "interface GigabitEthernet0/1",
                "no switchport trunk allowed vlan",
                "exit",
            ],
            "trunk_allowed",
        ),
        (vec!["no username admin"], "access_user"),
        (
            vec!["no ip route 0.0.0.0 0.0.0.0 10.0.0.1"],
            "default_route",
        ),
        (vec!["no ip routing"], "ip_routing"),
    ] {
        let change = planned(
            app,
            &lab.id("SW1"),
            json!({"type": "lines", "lines": lines}),
            json!({}),
        )
        .await;
        assert!(
            hit_kinds(&change).contains(&expected.to_string()),
            "{lines:?} should be a `{expected}` hit, got {:?}",
            hit_kinds(&change)
        );
    }
}

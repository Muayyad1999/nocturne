//! The built-in QoS templates: what each renders, what each refuses, that a
//! simulated IOS router takes the IOS ones line for line, and that applying
//! one makes a change and sends nothing.
//!
//! Nothing here is output recorded from a device. The expected lines follow
//! the vendors' own documentation, read on 2026-11-22:
//!
//! * `class-map` / `policy-map` / `service-policy output`, a shaper on
//!   `class-default` with a child policy inside it — Cisco, QoS Modular QoS
//!   Command-Line Interface Configuration Guide, "Applying QoS Features Using
//!   the MQC":
//!   <https://www.cisco.com/c/en/us/td/docs/ios-xml/ios/qos_mqc/configuration/xe-16/qos-mqc-xe-16-book/qos-apply.html>
//! * `priority percent` next to `bandwidth percent`, and "cannot exceed 99
//!   percent" — Cisco, QoS: Congestion Management Configuration Guide, "Low
//!   Latency Queueing with Priority Percentage Support":
//!   <https://www.cisco.com/c/en/us/td/docs/ios-xml/ios/qos_conmgt/configuration/xe-16/qos-conmgt-xe-16-book/qos-conmgt-llq-pps.html>
//! * `match dscp` (eight values, the names), `shape average <bps>` — Cisco IOS
//!   Quality of Service Solutions Command Reference:
//!   <https://www.cisco.com/c/en/us/td/docs/ios-xml/ios/qos/command/qos-cr-book/qos-m1.html>,
//!   <https://www.cisco.com/c/en/us/td/docs/ios-xml/ios/qos/command/qos-cr-book/qos-s1.html>
//! * `mls qos`, `mls qos trust dscp`, `set dscp` in `class-default` with
//!   `service-policy input` — Catalyst 3750 Software Configuration Guide,
//!   Release 12.2(55)SE, and Catalyst 2960 and 2960-S Software Configuration
//!   Guide, 12.2(55)SE, both "Configuring QoS":
//!   <https://www.cisco.com/c/en/us/td/docs/switches/lan/catalyst3750/software/release/12-2_55_se/configuration/guide/scg3750/swqos.html>,
//!   <https://www.cisco.com/c/en/us/td/docs/switches/lan/catalyst2960/software/release/12-2_55_se/configuration/guide/scg_2960/swqos.html>
//! * `/queue simple add … target= max-limit= limit-at=` — MikroTik, "Queues":
//!   <https://help.mikrotik.com/docs/spaces/ROS/pages/328088/Queues>
//! * `/queue tree add name= parent= packet-mark= limit-at= max-limit=
//!   priority=` and `mark-packet … passthrough=no` — MikroTik, "Connection
//!   rate" (its traffic prioritisation example):
//!   <https://help.mikrotik.com/docs/spaces/ROS/pages/131366985/Connection+rate>
//! * the `dscp` and `out-interface` matchers — MikroTik, "Common Firewall
//!   Matchers and Actions":
//!   <https://help.mikrotik.com/docs/spaces/ROS/pages/250708064/Common+Firewall+Matchers+and+Actions>

mod common;

use std::collections::BTreeSet;

use common::{Changes, lab};
use nocturne_app::testing::Lab;
use nocturne_drivers::hier::{ConfigTree, Node, Syntax};
use nocturne_netsim::Lab as SimLab;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

const QOS_TEMPLATES: [&str; 5] = [
    "ios_qos_wan_edge",
    "ios_qos_shaped_uplink",
    "ios_qos_access_port_dscp",
    "routeros_qos_simple_queue",
    "routeros_qos_queue_tree",
];

async fn render(lab: &Lab, id: &str, vars: Value) -> Value {
    lab.app
        .rpc(
            "templates.render",
            json!({"template_id": id, "device_ids": [lab.id("SW1")], "vars": vars}),
        )
        .await
        .unwrap_or_else(|err| panic!("{id}: {err}"))[0]
        .clone()
}

/// The lines a template renders to; the test fails if it refuses.
async fn lines(lab: &Lab, id: &str, vars: Value) -> Vec<String> {
    let rendered = render(lab, id, vars).await;
    assert_eq!(rendered["ok"], json!(true), "{id}: {:?}", rendered["error"]);
    rendered["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .map(|line| line.as_str().expect("a line of text").to_string())
        .collect()
}

/// What a template says when it refuses; the test fails if it renders.
async fn refusal(lab: &Lab, id: &str, vars: Value) -> String {
    let rendered = render(lab, id, vars.clone()).await;
    assert_eq!(
        rendered["ok"],
        json!(false),
        "{id} rendered {vars} to {:?}",
        rendered["lines"]
    );
    assert!(
        rendered["lines"].is_null(),
        "{id}: a refusal renders no line"
    );
    rendered["error"].as_str().expect("an error").to_string()
}

/// What each template needs and no more: every other variable is its default.
fn required_only(id: &str) -> Value {
    match id {
        "ios_qos_wan_edge" => json!({"interface": "Gi0/1"}),
        "ios_qos_shaped_uplink" => json!({"interface": "Gi0/1", "rate_kbps": 50000}),
        "ios_qos_access_port_dscp" => json!({"interface": "Gi1/0/5"}),
        "routeros_qos_simple_queue" => json!({
            "queue_name": "sub-1042", "target": "192.168.88.251/32",
            "upload_kbps": 5000, "download_kbps": 15000
        }),
        "routeros_qos_queue_tree" => json!({"interface": "ether1", "rate_kbps": 20000}),
        other => panic!("no variables written down for `{other}`"),
    }
}

#[tokio::test]
async fn every_qos_template_renders_with_its_defaults() {
    let lab = lab(&["SW1"]).await;
    let expected: [(&str, Vec<&str>); 5] = [
        (
            "ios_qos_wan_edge",
            vec![
                "class-map match-any WAN-EDGE-VOICE",
                " match dscp ef",
                "class-map match-any WAN-EDGE-VIDEO",
                " match dscp af41 af42 af43",
                "class-map match-any WAN-EDGE-CRITICAL",
                " match dscp af21 af22 af23 cs3",
                "policy-map WAN-EDGE",
                " class WAN-EDGE-VOICE",
                "  priority percent 10",
                "  exit",
                " class WAN-EDGE-VIDEO",
                "  bandwidth percent 20",
                "  exit",
                " class WAN-EDGE-CRITICAL",
                "  bandwidth percent 25",
                "interface GigabitEthernet0/1",
                " service-policy output WAN-EDGE",
            ],
        ),
        (
            "ios_qos_shaped_uplink",
            vec![
                "policy-map SHAPE-50000K",
                " class class-default",
                "  shape average 50000000",
                "interface GigabitEthernet0/1",
                " service-policy output SHAPE-50000K",
            ],
        ),
        (
            "ios_qos_access_port_dscp",
            vec!["interface GigabitEthernet1/0/5", " mls qos trust dscp"],
        ),
        (
            "routeros_qos_simple_queue",
            vec!["/queue simple add name=sub-1042 target=192.168.88.251/32 max-limit=5000k/15000k"],
        ),
        (
            "routeros_qos_queue_tree",
            vec![
                "/queue tree add name=wan parent=ether1 max-limit=20000k",
                "/queue tree add name=wan-voice parent=wan packet-mark=wan-voice limit-at=2000k max-limit=20000k priority=1",
                "/queue tree add name=wan-video parent=wan packet-mark=wan-video limit-at=4000k max-limit=20000k priority=3",
                "/queue tree add name=wan-critical parent=wan packet-mark=wan-critical limit-at=5000k max-limit=20000k priority=5",
                "/queue tree add name=wan-best-effort parent=wan packet-mark=wan-best-effort limit-at=9000k max-limit=20000k priority=8",
                "/ip firewall mangle add chain=postrouting out-interface=ether1 action=mark-packet new-packet-mark=wan-best-effort passthrough=yes comment=wan-qos",
                "/ip firewall mangle add chain=postrouting out-interface=ether1 dscp=46 action=mark-packet new-packet-mark=wan-voice passthrough=no comment=wan-qos",
                "/ip firewall mangle add chain=postrouting out-interface=ether1 dscp=34 action=mark-packet new-packet-mark=wan-video passthrough=no comment=wan-qos",
                "/ip firewall mangle add chain=postrouting out-interface=ether1 dscp=36 action=mark-packet new-packet-mark=wan-video passthrough=no comment=wan-qos",
                "/ip firewall mangle add chain=postrouting out-interface=ether1 dscp=38 action=mark-packet new-packet-mark=wan-video passthrough=no comment=wan-qos",
                "/ip firewall mangle add chain=postrouting out-interface=ether1 dscp=18 action=mark-packet new-packet-mark=wan-critical passthrough=no comment=wan-qos",
                "/ip firewall mangle add chain=postrouting out-interface=ether1 dscp=20 action=mark-packet new-packet-mark=wan-critical passthrough=no comment=wan-qos",
                "/ip firewall mangle add chain=postrouting out-interface=ether1 dscp=22 action=mark-packet new-packet-mark=wan-critical passthrough=no comment=wan-qos",
                "/ip firewall mangle add chain=postrouting out-interface=ether1 dscp=24 action=mark-packet new-packet-mark=wan-critical passthrough=no comment=wan-qos",
            ],
        ),
    ];
    for (id, wanted) in &expected {
        assert_eq!(
            lines(&lab, id, required_only(id)).await,
            *wanted,
            "{id} with its defaults"
        );
    }
    assert_eq!(
        expected.map(|(id, _)| id),
        QOS_TEMPLATES,
        "every QoS template is in the table above"
    );
}

#[tokio::test]
async fn the_qos_templates_are_built_ins_that_say_where_their_syntax_comes_from() {
    let lab = lab(&["SW1"]).await;
    let listed = lab
        .app
        .rpc("templates.list", json!({}))
        .await
        .expect("templates.list");
    let tagged: Vec<&Value> = listed
        .as_array()
        .expect("array")
        .iter()
        .filter(|template| template["tags"] == json!(["qos"]))
        .collect();
    let mut ids: Vec<&str> = tagged
        .iter()
        .map(|template| template["id"].as_str().expect("id"))
        .collect();
    ids.sort_unstable();
    let mut wanted = QOS_TEMPLATES;
    wanted.sort_unstable();
    assert_eq!(ids, wanted, "the templates tagged `qos`");

    for template in tagged {
        let id = template["id"].as_str().expect("id");
        assert_eq!(template["builtin"], json!(true), "{id}");
        assert_eq!(template["kind"], json!("config"), "{id}");
        let (profile, documentation) = if id.starts_with("ios_") {
            ("cisco_ios", "https://www.cisco.com/")
        } else {
            ("mikrotik_routeros", "https://help.mikrotik.com/")
        };
        assert_eq!(template["profiles"], json!([profile]), "{id}");
        assert!(
            template["description"]
                .as_str()
                .expect("description")
                .contains(documentation),
            "{id} does not cite the page its syntax comes from"
        );
        // A variable nobody explained is a variable somebody fills in wrongly.
        for variable in template["variables"].as_array().expect("variables") {
            assert!(
                !variable["label"].as_str().unwrap_or_default().is_empty(),
                "{id}: {variable}"
            );
        }
        // What `templates.save` checks: a copy somebody edits must still be
        // accepted, so the body may read nothing but its own variables.
        let checked = lab
            .app
            .rpc(
                "templates.validate",
                json!({"body": template["body"], "variables": template["variables"]}),
            )
            .await
            .expect("templates.validate");
        assert_eq!(checked["ok"], json!(true), "{id}: {checked}");
        assert_eq!(checked["undeclared"], json!([]), "{id}");
    }

    // Read-only, and copyable.
    let err = lab
        .app
        .rpc(
            "templates.save",
            json!({"id": "ios_qos_wan_edge", "name": "Mine", "body": "!"}),
        )
        .await
        .expect_err("a built-in cannot be overwritten");
    assert_eq!(err.code, "forbidden");
    let err = lab
        .app
        .rpc("templates.delete", json!({"id": "routeros_qos_queue_tree"}))
        .await
        .expect_err("a built-in cannot be deleted");
    assert_eq!(err.code, "forbidden");
    let copy = lab
        .app
        .rpc("templates.duplicate", json!({"id": "ios_qos_wan_edge"}))
        .await
        .expect("templates.duplicate");
    assert_eq!(copy["builtin"], json!(false));
    assert_eq!(copy["tags"], json!(["qos"]));

    // The copy keeps the checks: they are in the body, not beside it.
    let copied = lab
        .app
        .rpc(
            "templates.render",
            json!({"template_id": copy["id"], "device_ids": [lab.id("SW1")],
                   "vars": {"interface": "Gi0/1", "voice_percent": 60, "video_percent": 40}}),
        )
        .await
        .expect("templates.render")[0]
        .clone();
    assert_eq!(copied["ok"], json!(false));
    assert!(
        copied["error"]
            .as_str()
            .expect("an error")
            .contains("add up to 125 percent"),
        "{copied}"
    );
}

#[tokio::test]
async fn a_circuit_rate_puts_the_wan_policy_inside_a_shaper() {
    let lab = lab(&["SW1"]).await;
    let rendered = lines(
        &lab,
        "ios_qos_wan_edge",
        json!({
            "interface": "Gi0/1", "policy_name": "BRANCH", "shape_kbps": 50000,
            // What a text box gives: one string, mixed case, numbers and names.
            // They come out as the router writes them back: by name where
            // there is one, in ascending order.
            "voice_dscp": "EF, 44", "voice_percent": "15",
            "video_dscp": ["af41", 32], "video_percent": 30,
            "critical_dscp": ["cs3", "AF21", "default"], "critical_percent": 20,
        }),
    )
    .await;
    assert_eq!(
        rendered,
        vec![
            "class-map match-any BRANCH-VOICE",
            " match dscp 44 ef",
            "class-map match-any BRANCH-VIDEO",
            " match dscp cs4 af41",
            "class-map match-any BRANCH-CRITICAL",
            " match dscp default af21 cs3",
            "policy-map BRANCH",
            " class BRANCH-VOICE",
            "  priority percent 15",
            "  exit",
            " class BRANCH-VIDEO",
            "  bandwidth percent 30",
            "  exit",
            " class BRANCH-CRITICAL",
            "  bandwidth percent 20",
            "policy-map BRANCH-SHAPE",
            " class class-default",
            "  shape average 50000000",
            "  service-policy BRANCH",
            "interface GigabitEthernet0/1",
            " service-policy output BRANCH-SHAPE",
        ]
    );
}

#[tokio::test]
async fn an_access_port_either_trusts_the_marking_or_overwrites_it() {
    let lab = lab(&["SW1"]).await;
    assert_eq!(
        lines(
            &lab,
            "ios_qos_access_port_dscp",
            json!({"interface": "Gi1/0/5", "mode": "set", "dscp": "AF21"})
        )
        .await,
        vec![
            "policy-map MARK-DSCP-AF21",
            " class class-default",
            "  set dscp af21",
            "interface GigabitEthernet1/0/5",
            " service-policy input MARK-DSCP-AF21",
        ]
    );
    assert_eq!(
        lines(
            &lab,
            "ios_qos_access_port_dscp",
            json!({"interface": "Gi1/0/5", "mode": "set", "dscp": 0, "policy_name": "SCAVENGE"})
        )
        .await,
        vec![
            "policy-map SCAVENGE",
            " class class-default",
            "  set dscp default",
            "interface GigabitEthernet1/0/5",
            " service-policy input SCAVENGE",
        ]
    );
}

#[tokio::test]
async fn qos_is_enabled_for_the_whole_switch_only_when_that_is_asked_for() {
    // `mls qos` is not a port setting: once it is on, every port that is not
    // trusted rewrites DSCP to 0. A template for one access port must not
    // slip that in.
    let lab = lab(&["SW1"]).await;
    for vars in [
        json!({"interface": "Gi1/0/5"}),
        json!({"interface": "Gi1/0/5", "mode": "set", "dscp": "ef"}),
    ] {
        let rendered = lines(&lab, "ios_qos_access_port_dscp", vars).await;
        assert!(
            !rendered.iter().any(|line| line.trim() == "mls qos"),
            "{rendered:?}"
        );
    }
    assert_eq!(
        lines(
            &lab,
            "ios_qos_access_port_dscp",
            json!({"interface": "Gi1/0/5", "enable_qos": true})
        )
        .await,
        vec![
            "mls qos",
            "interface GigabitEthernet1/0/5",
            " mls qos trust dscp"
        ]
    );
}

#[tokio::test]
async fn a_subscriber_queue_can_guarantee_part_of_its_limit() {
    let lab = lab(&["SW1"]).await;
    assert_eq!(
        lines(
            &lab,
            "routeros_qos_simple_queue",
            json!({
                "queue_name": "sub-1042", "target": "10.20.0.17/32",
                "upload_kbps": 5000, "download_kbps": 15000,
                "guaranteed_upload_kbps": 1000, "guaranteed_download_kbps": 5000,
            })
        )
        .await,
        vec![
            "/queue simple add name=sub-1042 target=10.20.0.17/32 max-limit=5000k/15000k limit-at=1000k/5000k"
        ]
    );
}

#[tokio::test]
async fn the_classes_of_a_queue_tree_are_guaranteed_the_whole_uplink_between_them() {
    let lab = lab(&["SW1"]).await;
    let rendered = lines(
        &lab,
        "routeros_qos_queue_tree",
        json!({
            "interface": "pppoe-out1", "rate_kbps": 7777, "name_prefix": "up",
            "voice_dscp": "46", "voice_percent": 12,
            "video_dscp": [34], "video_percent": 33,
            "critical_dscp": [24], "critical_percent": 21,
        }),
    )
    .await;
    assert_eq!(
        rendered,
        vec![
            "/queue tree add name=up parent=pppoe-out1 max-limit=7777k",
            "/queue tree add name=up-voice parent=up packet-mark=up-voice limit-at=933k max-limit=7777k priority=1",
            "/queue tree add name=up-video parent=up packet-mark=up-video limit-at=2566k max-limit=7777k priority=3",
            "/queue tree add name=up-critical parent=up packet-mark=up-critical limit-at=1633k max-limit=7777k priority=5",
            "/queue tree add name=up-best-effort parent=up packet-mark=up-best-effort limit-at=2645k max-limit=7777k priority=8",
            "/ip firewall mangle add chain=postrouting out-interface=pppoe-out1 action=mark-packet new-packet-mark=up-best-effort passthrough=yes comment=up-qos",
            "/ip firewall mangle add chain=postrouting out-interface=pppoe-out1 dscp=46 action=mark-packet new-packet-mark=up-voice passthrough=no comment=up-qos",
            "/ip firewall mangle add chain=postrouting out-interface=pppoe-out1 dscp=34 action=mark-packet new-packet-mark=up-video passthrough=no comment=up-qos",
            "/ip firewall mangle add chain=postrouting out-interface=pppoe-out1 dscp=24 action=mark-packet new-packet-mark=up-critical passthrough=no comment=up-qos",
        ]
    );

    // MikroTik's rule for HTB: the guarantees of the children add up to no
    // more than the parent has. Here they add up to exactly that, whatever the
    // rounding of the shares did.
    let guaranteed: u64 = rendered
        .iter()
        .filter_map(|line| line.split(" limit-at=").nth(1))
        .map(|rest| {
            rest.split('k')
                .next()
                .expect("a rate")
                .parse::<u64>()
                .expect("a number")
        })
        .sum();
    assert_eq!(guaranteed, 7777);
}

#[tokio::test]
async fn a_qos_template_refuses_lines_a_device_would_take_and_then_queue_wrongly() {
    let lab = lab(&["SW1"]).await;
    let wan = |extra: Value| {
        let mut vars = json!({"interface": "Gi0/1"});
        vars.as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("object").clone());
        vars
    };
    let tree = |extra: Value| {
        let mut vars = json!({"interface": "ether1", "rate_kbps": 20000});
        vars.as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("object").clone());
        vars
    };
    let queue = |extra: Value| {
        let mut vars = required_only("routeros_qos_simple_queue");
        vars.as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("object").clone());
        vars
    };
    let cases: Vec<(&str, Value, &str)> = vec![
        // ---------------------------------------------------- IOS, WAN uplink
        (
            "ios_qos_wan_edge",
            wan(json!({"voice_percent": 40, "video_percent": 40, "critical_percent": 20})),
            "add up to 100 percent",
        ),
        (
            "ios_qos_wan_edge",
            wan(json!({"voice_percent": 0})),
            "Voice share must be between 1 and 100 percent",
        ),
        (
            "ios_qos_wan_edge",
            wan(json!({"critical_percent": 101})),
            "Critical data share must be between 1 and 100 percent",
        ),
        (
            "ios_qos_wan_edge",
            wan(json!({"voice_dscp": ["ef", "af41"]})),
            "DSCP `af41` is listed more than once",
        ),
        (
            "ios_qos_wan_edge",
            wan(json!({"video_dscp": ["af41", "AF41"]})),
            "DSCP `af41` is listed more than once",
        ),
        (
            "ios_qos_wan_edge",
            wan(json!({"voice_dscp": ["64"]})),
            "Voice DSCP: `64` is not a DSCP",
        ),
        (
            "ios_qos_wan_edge",
            wan(json!({"video_dscp": ["af44"]})),
            "Video DSCP: `af44` is not a DSCP",
        ),
        (
            "ios_qos_wan_edge",
            wan(json!({"critical_dscp": ["cs0"]})),
            "Critical data DSCP: `cs0` is not a DSCP",
        ),
        (
            "ios_qos_wan_edge",
            wan(json!({"critical_dscp": [1, 2, 3, 4, 5, 6, 7, 8, 9]})),
            "9 values were given, and one `match dscp` line takes eight at most",
        ),
        (
            "ios_qos_wan_edge",
            wan(json!({"policy_name": "WAN EDGE"})),
            "Policy name must be one word",
        ),
        (
            "ios_qos_wan_edge",
            wan(json!({"policy_name": "A-NAME-THAT-IS-THIRTY-TWO-LETTERS"})),
            "31 characters at most",
        ),
        (
            "ios_qos_wan_edge",
            wan(json!({"shape_kbps": 5})),
            "Circuit rate must be 0 (no shaper) or between 8 and 128000000 kbit/s",
        ),
        (
            "ios_qos_wan_edge",
            wan(json!({"shape_kbps": -50000})),
            "Circuit rate must be 0 (no shaper)",
        ),
        // A share of a small circuit can come to less than IOS takes for a
        // class: 20 percent of 39 kbit/s is 7.
        (
            "ios_qos_wan_edge",
            wan(json!({"shape_kbps": 39})),
            "Video share: 20 percent of a 39 kbit/s circuit is less than 8 kbit/s",
        ),
        (
            "ios_qos_wan_edge",
            wan(json!({"shape_kbps": 64, "voice_percent": 1})),
            "Voice share: 1 percent of a 64 kbit/s circuit is less than 1 kbit/s",
        ),
        // ------------------------------------------------- IOS, shaped uplink
        (
            "ios_qos_shaped_uplink",
            json!({"interface": "Gi0/1", "rate_kbps": 7}),
            "Circuit rate must be between 8 and 128000000 kbit/s, not 7",
        ),
        (
            "ios_qos_shaped_uplink",
            json!({"interface": "Gi0/1", "rate_kbps": 128000001}),
            "Circuit rate must be between 8 and 128000000 kbit/s",
        ),
        (
            "ios_qos_shaped_uplink",
            json!({"interface": "Gi0/1", "rate_kbps": 50000, "policy_name": "shape it"}),
            "Policy name must be one word",
        ),
        // --------------------------------------------------- IOS, access port
        (
            "ios_qos_access_port_dscp",
            json!({"interface": "Gi1/0/5", "mode": "set"}),
            "DSCP to set: give a number from 0 to 63",
        ),
        (
            "ios_qos_access_port_dscp",
            json!({"interface": "Gi1/0/5", "mode": "set", "dscp": "voice"}),
            "DSCP to set: give a number from 0 to 63",
        ),
        (
            "ios_qos_access_port_dscp",
            json!({"interface": "Gi1/0/5", "dscp": "ef"}),
            "belong to mode `set`",
        ),
        (
            "ios_qos_access_port_dscp",
            json!({"interface": "Gi1/0/5", "mode": "set", "dscp": "ef", "policy_name": "a b"}),
            "Policy name must be one word",
        ),
        // --------------------------------------------- RouterOS, simple queue
        (
            "routeros_qos_simple_queue",
            queue(json!({"guaranteed_upload_kbps": 6000, "guaranteed_download_kbps": 5000})),
            "Guaranteed upload (6000 kbit/s) is above the upload limit (5000 kbit/s)",
        ),
        (
            "routeros_qos_simple_queue",
            queue(json!({"guaranteed_upload_kbps": 1000, "guaranteed_download_kbps": 15001})),
            "Guaranteed download (15001 kbit/s) is above the download limit (15000 kbit/s)",
        ),
        (
            "routeros_qos_simple_queue",
            queue(json!({"guaranteed_download_kbps": 5000})),
            "Give both guaranteed rates or neither",
        ),
        (
            "routeros_qos_simple_queue",
            queue(json!({"download_kbps": 0})),
            "at least 1 kbit/s",
        ),
        (
            "routeros_qos_simple_queue",
            queue(json!({"target": "2001:db8::17/128"})),
            "Subscriber address must be an IPv4 address",
        ),
        (
            "routeros_qos_simple_queue",
            queue(json!({"queue_name": "sub 1042 disabled=yes"})),
            "Queue name must be one word",
        ),
        // ----------------------------------------------- RouterOS, queue tree
        (
            "routeros_qos_queue_tree",
            tree(json!({"voice_percent": 50, "video_percent": 30, "critical_percent": 20})),
            "add up to 100 percent",
        ),
        (
            "routeros_qos_queue_tree",
            tree(json!({"voice_dscp": ["ef"]})),
            "Voice DSCP: `ef` is not a DSCP number",
        ),
        (
            "routeros_qos_queue_tree",
            tree(json!({"video_dscp": [64]})),
            "Video DSCP: `64` is not a DSCP number",
        ),
        (
            "routeros_qos_queue_tree",
            tree(json!({"critical_dscp": [46, 24]})),
            "DSCP 46 is listed more than once",
        ),
        (
            "routeros_qos_queue_tree",
            tree(json!({"rate_kbps": 9})),
            "An uplink rate of 9 kbit/s is too small to divide",
        ),
        (
            "routeros_qos_queue_tree",
            tree(json!({"interface": "ether1 disabled=yes"})),
            "Uplink interface must be the interface's RouterOS name",
        ),
        (
            "routeros_qos_queue_tree",
            tree(json!({"name_prefix": "wan uplink"})),
            "Name prefix must be one word",
        ),
        (
            "routeros_qos_queue_tree",
            tree(json!({"video_percent": 0})),
            "Video share must be between 1 and 100 percent",
        ),
    ];
    // Every case is tried before the test fails, so that one run names all
    // the refusals that are broken and not only the first.
    let mut wrong: Vec<String> = Vec::new();
    for (id, vars, wanted) in &cases {
        let rendered = render(&lab, id, vars.clone()).await;
        match rendered["error"].as_str() {
            Some(said) if rendered["ok"] == json!(false) && rendered["lines"].is_null() => {
                // The template's own words, not a report of a broken template.
                if !said.contains(wanted) || said.contains("could not be rendered") {
                    wrong.push(format!("{id} {vars} said: {said}"));
                }
            }
            _ => wrong.push(format!("{id} rendered {vars} to {}", rendered["lines"])),
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    for id in QOS_TEMPLATES {
        assert!(
            cases.iter().any(|(covered, ..)| *covered == id),
            "`{id}` has no refusal in the table above"
        );
    }
}

#[tokio::test]
async fn a_value_of_the_wrong_kind_is_refused_before_anything_is_rendered() {
    let lab = lab(&["SW1"]).await;
    for (id, vars, wanted) in [
        (
            "ios_qos_shaped_uplink",
            json!({"interface": "Gi0/1"}),
            "`Circuit rate (kbit/s)` is required",
        ),
        (
            "ios_qos_shaped_uplink",
            json!({"interface": "Gi0/1", "rate_kbps": "fifty"}),
            "`Circuit rate (kbit/s)` must be a whole number",
        ),
        ("ios_qos_wan_edge", json!({}), "`WAN interface` is required"),
        (
            "ios_qos_access_port_dscp",
            json!({"interface": "Gi1/0/5", "mode": "police"}),
            "`Mode` must be one of trust, set",
        ),
        (
            "routeros_qos_simple_queue",
            json!({"queue_name": "sub-1042", "target": "192.168.88.251",
                   "upload_kbps": 5000, "download_kbps": 15000}),
            "`Subscriber address` must be an address with a prefix length",
        ),
        (
            "routeros_qos_queue_tree",
            json!({"interface": "ether1"}),
            "`Uplink rate (kbit/s)` is required",
        ),
    ] {
        let err = lab
            .app
            .rpc(
                "templates.render",
                json!({"template_id": id, "device_ids": [lab.id("SW1")], "vars": vars}),
            )
            .await
            .expect_err("the variable is not of its kind");
        assert_eq!(err.code, "invalid", "{id}");
        assert!(err.message.contains(wanted), "{id}: {}", err.message);
    }
}

/// What is said when a value is turned away: by its kind, before anything is
/// rendered (the call fails), or by the template (that device's `error`).
/// `Err` says what happened instead, a rendered line above all.
async fn turned_away(lab: &Lab, id: &str, vars: &Value) -> Result<String, String> {
    // A megabyte of variable in a failure message helps nobody.
    let shown: String = vars.to_string().chars().take(200).collect();
    let outcome = lab
        .app
        .rpc(
            "templates.render",
            json!({"template_id": id, "device_ids": [lab.id("SW1")], "vars": vars}),
        )
        .await;
    match outcome {
        Err(err) if err.code == "invalid" => Ok(err.message),
        Err(err) => Err(format!(
            "{id} {shown} failed with `{}`: {}",
            err.code, err.message
        )),
        Ok(rendered) => {
            let rendered = &rendered[0];
            match rendered["error"].as_str() {
                Some(said) if rendered["ok"] == json!(false) && rendered["lines"].is_null() => {
                    Ok(said.to_string())
                }
                _ => {
                    let lines: String = rendered["lines"].to_string().chars().take(300).collect();
                    Err(format!("{id} rendered {shown} to {lines}"))
                }
            }
        }
    }
}

/// The variables every template needs, with `extra` on top.
fn with(id: &str, extra: Value) -> Value {
    let mut vars = required_only(id);
    vars.as_object_mut()
        .expect("object")
        .extend(extra.as_object().expect("object").clone());
    vars
}

#[tokio::test]
async fn a_hostile_value_is_turned_away_and_never_becomes_a_line() {
    let lab = lab(&["SW1"]).await;
    let megabyte = "A".repeat(1_000_000);
    let most = i64::MAX;
    let cases: Vec<(&str, Value)> = vec![
        // -------------------------------------------- a name that is not one
        (
            "ios_qos_wan_edge",
            json!({"policy_name": "WAN\nno ip routing"}),
        ),
        ("ios_qos_wan_edge", json!({"policy_name": "WAN\tEDGE"})),
        ("ios_qos_wan_edge", json!({"policy_name": "WAN;EDGE"})),
        (
            "ios_qos_wan_edge",
            json!({"policy_name": "WAN\u{1b}[31mEDGE"}),
        ),
        ("ios_qos_wan_edge", json!({"policy_name": megabyte})),
        (
            "ios_qos_shaped_uplink",
            json!({"policy_name": "UPLINK\n class class-default"}),
        ),
        (
            "ios_qos_shaped_uplink",
            json!({"policy_name": "A".repeat(41)}),
        ),
        (
            "ios_qos_access_port_dscp",
            json!({"mode": "set", "dscp": "ef", "policy_name": "MARK\nmls qos"}),
        ),
        (
            "ios_qos_access_port_dscp",
            json!({"mode": "set", "dscp": "ef", "policy_name": "A".repeat(41)}),
        ),
        (
            "routeros_qos_simple_queue",
            json!({"queue_name": "sub\n/system reboot"}),
        ),
        (
            "routeros_qos_simple_queue",
            json!({"queue_name": "[/system reboot]"}),
        ),
        (
            "routeros_qos_simple_queue",
            json!({"queue_name": "sub;/system reboot"}),
        ),
        (
            "routeros_qos_simple_queue",
            json!({"queue_name": "\"sub 1\""}),
        ),
        ("routeros_qos_simple_queue", json!({"queue_name": "$sub"})),
        ("routeros_qos_simple_queue", json!({"queue_name": "sub=1"})),
        ("routeros_qos_simple_queue", json!({"queue_name": megabyte})),
        (
            "routeros_qos_queue_tree",
            json!({"name_prefix": "wan\n/system reboot"}),
        ),
        ("routeros_qos_queue_tree", json!({"name_prefix": "wan=1"})),
        (
            "routeros_qos_queue_tree",
            json!({"name_prefix": "[/system reboot]"}),
        ),
        (
            "routeros_qos_queue_tree",
            json!({"name_prefix": "w".repeat(33)}),
        ),
        // The parent queue takes the prefix as its name, and the classes name
        // their parent: a queue called like the interface, or like RouterOS's
        // own `global`, is two things under one name.
        (
            "routeros_qos_queue_tree",
            json!({"interface": "wan", "name_prefix": "WAN"}),
        ),
        ("routeros_qos_queue_tree", json!({"name_prefix": "global"})),
        // ------------------------------------- an interface that is not one
        ("ios_qos_wan_edge", json!({"interface": "Gi0/1\n shutdown"})),
        ("ios_qos_wan_edge", json!({"interface": "Gi0/1 shutdown"})),
        ("ios_qos_wan_edge", json!({"interface": "range Gi0/1 - 24"})),
        ("ios_qos_wan_edge", json!({"interface": "Gi0/1;shutdown"})),
        (
            "ios_qos_wan_edge",
            json!({"interface": "<script>alert(1)</script>"}),
        ),
        ("ios_qos_wan_edge", json!({"interface": megabyte})),
        (
            "ios_qos_shaped_uplink",
            json!({"interface": "Gi0/1 shutdown"}),
        ),
        (
            "ios_qos_shaped_uplink",
            json!({"interface": "range Gi0/1 - 24"}),
        ),
        (
            "ios_qos_access_port_dscp",
            json!({"interface": "Gi1/0/5 shutdown"}),
        ),
        (
            "ios_qos_access_port_dscp",
            json!({"interface": "range Gi1/0/1 - 48"}),
        ),
        (
            "ios_qos_access_port_dscp",
            json!({"interface": "Gi1/0/5", "mode": "set\n mls qos"}),
        ),
        (
            "routeros_qos_queue_tree",
            json!({"interface": "ether1\n/system reboot"}),
        ),
        (
            "routeros_qos_queue_tree",
            json!({"interface": "[/system reboot]"}),
        ),
        ("routeros_qos_queue_tree", json!({"interface": "ether1;"})),
        ("routeros_qos_queue_tree", json!({"interface": megabyte})),
        // ------------------------------------------ a marking that is not one
        (
            "ios_qos_wan_edge",
            json!({"voice_dscp": ["ef\n match any"]}),
        ),
        ("ios_qos_wan_edge", json!({"voice_dscp": ["ef 46"]})),
        ("ios_qos_wan_edge", json!({"voice_dscp": [[46]]})),
        ("ios_qos_wan_edge", json!({"voice_dscp": [46.5]})),
        ("ios_qos_wan_edge", json!({"voice_dscp": [true]})),
        ("ios_qos_wan_edge", json!({"voice_dscp": [-1]})),
        ("ios_qos_wan_edge", json!({"voice_dscp": ["0x2e"]})),
        ("ios_qos_wan_edge", json!({"voice_dscp": {"ef": 46}})),
        ("ios_qos_wan_edge", json!({"voice_dscp": [megabyte]})),
        (
            "ios_qos_access_port_dscp",
            json!({"mode": "set", "dscp": "ef\n service-policy output X"}),
        ),
        (
            "ios_qos_access_port_dscp",
            json!({"mode": "set", "dscp": "ef af41"}),
        ),
        (
            "ios_qos_access_port_dscp",
            json!({"mode": "set", "dscp": -1}),
        ),
        (
            "ios_qos_access_port_dscp",
            json!({"mode": "set", "dscp": 46.5}),
        ),
        (
            "routeros_qos_queue_tree",
            json!({"voice_dscp": ["46 passthrough=yes"]}),
        ),
        ("routeros_qos_queue_tree", json!({"voice_dscp": [46.5]})),
        ("routeros_qos_queue_tree", json!({"voice_dscp": [-1]})),
        ("routeros_qos_queue_tree", json!({"voice_dscp": ["046"]})),
        ("routeros_qos_queue_tree", json!({"voice_dscp": [megabyte]})),
        // -------------------------------------------- a rate that is not one
        ("ios_qos_wan_edge", json!({"shape_kbps": -1})),
        ("ios_qos_wan_edge", json!({"shape_kbps": most})),
        ("ios_qos_wan_edge", json!({"shape_kbps": 50000.5})),
        ("ios_qos_shaped_uplink", json!({"rate_kbps": -50000})),
        ("ios_qos_shaped_uplink", json!({"rate_kbps": 0})),
        ("ios_qos_shaped_uplink", json!({"rate_kbps": most})),
        ("ios_qos_shaped_uplink", json!({"rate_kbps": "50000 kbit"})),
        ("routeros_qos_simple_queue", json!({"upload_kbps": -5000})),
        ("routeros_qos_simple_queue", json!({"download_kbps": most})),
        (
            "routeros_qos_simple_queue",
            json!({"upload_kbps": 128_000_001}),
        ),
        (
            "routeros_qos_simple_queue",
            json!({"guaranteed_upload_kbps": -1, "guaranteed_download_kbps": -1}),
        ),
        ("routeros_qos_queue_tree", json!({"rate_kbps": -20000})),
        ("routeros_qos_queue_tree", json!({"rate_kbps": 0})),
        ("routeros_qos_queue_tree", json!({"rate_kbps": most})),
        ("routeros_qos_queue_tree", json!({"rate_kbps": 128_000_001})),
        // ------------------------------------------- a share that is not one
        ("ios_qos_wan_edge", json!({"voice_percent": -5})),
        ("ios_qos_wan_edge", json!({"voice_percent": most})),
        ("ios_qos_wan_edge", json!({"voice_percent": 10.5})),
        ("ios_qos_wan_edge", json!({"voice_percent": [10]})),
        (
            "ios_qos_wan_edge",
            json!({"voice_percent": 34, "video_percent": 33, "critical_percent": 33}),
        ),
        ("routeros_qos_queue_tree", json!({"voice_percent": -1})),
        (
            "routeros_qos_queue_tree",
            json!({"voice_percent": 33, "video_percent": 33, "critical_percent": 34}),
        ),
        // --------------------------------------- an address that is not one
        (
            "routeros_qos_simple_queue",
            json!({"target": "192.168.88.251/32 disabled=yes"}),
        ),
        (
            "routeros_qos_simple_queue",
            json!({"target": "192.168.88.251/32\n/system reset-configuration"}),
        ),
        (
            "routeros_qos_simple_queue",
            json!({"target": "[:resolve x]/32"}),
        ),
        (
            "routeros_qos_simple_queue",
            json!({"target": "10.0.0.1/+32"}),
        ),
        (
            "routeros_qos_simple_queue",
            json!({"target": "10.0.0.1/33"}),
        ),
    ];
    // Every case is tried before the test fails, so that one run names all
    // the values that got through and not only the first.
    let mut wrong: Vec<String> = Vec::new();
    for (id, extra) in &cases {
        let said = match turned_away(&lab, id, &with(id, extra.clone())).await {
            Ok(said) => said,
            Err(what) => {
                wrong.push(what);
                continue;
            }
        };
        // What comes back is one line of text a person can read: the value is
        // quoted at most in part, and nothing in it moves a cursor or starts
        // a new line in a log.
        let start: String = said.chars().take(120).collect();
        if said.chars().count() > 600 {
            wrong.push(format!(
                "{id}: a refusal of {} characters: {start}",
                said.chars().count()
            ));
        }
        if said.contains(char::is_control) {
            wrong.push(format!("{id}: a control character in {start:?}"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    for id in QOS_TEMPLATES {
        assert!(
            cases.iter().any(|(covered, _)| *covered == id),
            "`{id}` has no hostile value in the table above"
        );
    }
}

#[tokio::test]
async fn a_copy_without_its_defaults_still_refuses_a_class_with_no_marking() {
    // The built-ins fill an empty list in from the default, so only a copy
    // can get here: somebody duplicates the template, drops the defaults, and
    // leaves a class empty. `match dscp` with nothing after it is not a line.
    let lab = lab(&["SW1"]).await;
    for (id, vars, label) in [
        (
            "ios_qos_wan_edge",
            json!({"interface": "Gi0/1", "policy_name": "EDGE", "voice_dscp": ["ef"],
                   "voice_percent": 10, "video_dscp": [], "video_percent": 20,
                   "critical_dscp": ["af21"], "critical_percent": 25, "shape_kbps": 0}),
            "Video DSCP",
        ),
        (
            "routeros_qos_queue_tree",
            json!({"interface": "ether1", "rate_kbps": 20000, "name_prefix": "up",
                   "voice_dscp": [46], "voice_percent": 10, "video_dscp": [34],
                   "video_percent": 20, "critical_dscp": [], "critical_percent": 25}),
            "Critical data DSCP",
        ),
    ] {
        let original = lab
            .app
            .rpc("templates.get", json!({"id": id}))
            .await
            .expect("templates.get");
        let mut variables = original["variables"].clone();
        for variable in variables.as_array_mut().expect("variables") {
            variable["default"] = Value::Null;
        }
        let rendered = lab
            .app
            .rpc(
                "templates.render",
                json!({"body": original["body"], "variables": variables,
                       "device_ids": [lab.id("SW1")], "vars": vars}),
            )
            .await
            .expect("templates.render")[0]
            .clone();
        assert_eq!(
            rendered["ok"],
            json!(false),
            "{id}: {:?}",
            rendered["lines"]
        );
        assert_eq!(
            rendered["error"],
            json!(format!("{label}: give at least one DSCP")),
            "{id}"
        );
    }
}

#[tokio::test]
async fn a_marking_named_and_numbered_is_one_marking_listed_twice() {
    // `ef` and 46 are the same six bits. A router takes both lines and puts
    // the packet in whichever class comes first; the other class never sees it.
    let lab = lab(&["SW1"]).await;
    for (vars, word) in [
        (json!({"voice_dscp": [46], "video_dscp": ["ef"]}), "ef"),
        (
            json!({"voice_dscp": ["0"], "critical_dscp": ["default"]}),
            "default",
        ),
        (json!({"voice_dscp": [34]}), "af41"),
        (json!({"critical_dscp": ["cs3", 24]}), "cs3"),
        (json!({"voice_dscp": "ef;46"}), "ef"),
        (json!({"video_dscp": ["AF41", "34"]}), "af41"),
        (json!({"voice_dscp": [5], "critical_dscp": ["5"]}), "5"),
    ] {
        let said = refusal(
            &lab,
            "ios_qos_wan_edge",
            with("ios_qos_wan_edge", vars.clone()),
        )
        .await;
        assert!(
            said.contains(&format!("DSCP `{word}` is listed more than once")),
            "{vars}: {said}"
        );
    }
}

#[tokio::test]
async fn a_marking_is_rendered_the_way_the_router_writes_it_back() {
    // The change made from these lines is checked against the device
    // afterwards, line for line. A router stores the code point, not what
    // was typed: it shows 46 as `ef` and a class map's values in ascending
    // order, so that is what has to be rendered.
    let lab = lab(&["SW1"]).await;
    let rendered = lines(
        &lab,
        "ios_qos_wan_edge",
        json!({
            "interface": "Gi0/1",
            "voice_dscp": ["EF", 44],
            "video_dscp": [38, "af41", "36", 32],
            "critical_dscp": ["cs3", 18, "DEFAULT", 7, "af23"],
        }),
    )
    .await;
    let matches: Vec<&str> = rendered
        .iter()
        .filter(|line| line.starts_with(" match dscp"))
        .map(String::as_str)
        .collect();
    assert_eq!(
        matches,
        vec![
            " match dscp 44 ef",
            " match dscp cs4 af41 af42 af43",
            " match dscp default 7 af21 af23 cs3",
        ]
    );

    for (dscp, wanted, name) in [
        (json!(46), "  set dscp ef", "MARK-DSCP-EF"),
        (json!("0"), "  set dscp default", "MARK-DSCP-DEFAULT"),
        (json!(8), "  set dscp cs1", "MARK-DSCP-CS1"),
        (json!("AF33"), "  set dscp af33", "MARK-DSCP-AF33"),
        (json!(63), "  set dscp 63", "MARK-DSCP-63"),
    ] {
        let rendered = lines(
            &lab,
            "ios_qos_access_port_dscp",
            json!({"interface": "Gi1/0/5", "mode": "set", "dscp": dscp}),
        )
        .await;
        assert_eq!(
            rendered,
            vec![
                format!("policy-map {name}"),
                " class class-default".to_string(),
                wanted.to_string(),
                "interface GigabitEthernet1/0/5".to_string(),
                format!(" service-policy input {name}"),
            ],
            "dscp {dscp}"
        );
    }
}

#[tokio::test]
async fn every_code_point_is_written_the_way_the_simulated_router_writes_it() {
    // The names in the template's body and the names in the simulator are two
    // tables written by two hands. For each of the 64 code points, given as a
    // number, the line the template renders must be the line the router shows.
    let lab = lab(&["SW1"]).await;
    for code in 0..=63 {
        let rendered = lines(
            &lab,
            "ios_qos_access_port_dscp",
            json!({"interface": "Gi0/0", "mode": "set", "dscp": code}),
        )
        .await;
        let (mut sim, r1) = router();
        typed(&mut sim, r1, &rendered);
        let running = sim.running_config(r1).expect("running-config");
        let tree = ConfigTree::parse(&running, Syntax::Ios);
        let path = [
            rendered[0].as_str(),
            "class class-default",
            rendered[2].trim(),
        ];
        assert!(
            tree.find(&path).is_some(),
            "DSCP {code}: {path:?} is not in:\n{running}"
        );
    }
}

#[tokio::test]
async fn an_interface_is_named_as_the_router_names_it() {
    let lab = lab(&["SW1"]).await;
    for (given, wanted) in [
        ("gi 0/1", "interface GigabitEthernet0/1"),
        (
            "GigabitEthernet0/0/0.100",
            "interface GigabitEthernet0/0/0.100",
        ),
        ("Serial0/0/0:23", "interface Serial0/0/0:23"),
        ("po1", "interface Port-channel1"),
        ("Dialer1", "interface Dialer1"),
        ("Dot11Radio0", "interface Dot11Radio0"),
    ] {
        let rendered = lines(
            &lab,
            "ios_qos_shaped_uplink",
            json!({"interface": given, "rate_kbps": 2000}),
        )
        .await;
        assert_eq!(rendered[3], wanted, "{given}");
    }
}

// ----------------------------------------------------------- the simulator

/// A simulated IOS router with nothing configured on it.
fn router() -> (SimLab, u32) {
    let mut sim = SimLab::new("qos-templates");
    let node = sim
        .add_node("router", Some("R1"), 0.0, 0.0)
        .expect("a router");
    (sim, node)
}

/// Type the lines at the router's configuration prompt; every one must be taken.
fn typed(sim: &mut SimLab, node: u32, lines: &[String]) {
    let outcome = sim.configure(node, lines).expect("configure");
    let refused: Vec<String> = outcome
        .lines
        .iter()
        .filter(|line| line.error)
        .map(|line| format!("{} -> {}", line.line, line.output.trim()))
        .collect();
    assert!(
        refused.is_empty() && outcome.ok,
        "the simulated router refused: {refused:#?}"
    );
}

#[tokio::test]
async fn a_simulated_router_takes_the_wan_policy_line_for_line() {
    let lab = lab(&["SW1"]).await;
    let (mut sim, r1) = router();
    let rendered = lines(&lab, "ios_qos_wan_edge", json!({"interface": "Gi0/1"})).await;
    typed(&mut sim, r1, &rendered);

    // Every line is in the configuration the router now shows, where it was put.
    let running = sim.running_config(r1).expect("running-config");
    let tree = ConfigTree::parse(&running, Syntax::Ios);
    for path in [
        vec!["class-map match-any WAN-EDGE-VOICE", "match dscp ef"],
        vec![
            "class-map match-any WAN-EDGE-VIDEO",
            "match dscp af41 af42 af43",
        ],
        vec![
            "class-map match-any WAN-EDGE-CRITICAL",
            "match dscp af21 af22 af23 cs3",
        ],
        vec![
            "policy-map WAN-EDGE",
            "class WAN-EDGE-VOICE",
            "priority percent 10",
        ],
        vec![
            "policy-map WAN-EDGE",
            "class WAN-EDGE-VIDEO",
            "bandwidth percent 20",
        ],
        vec![
            "policy-map WAN-EDGE",
            "class WAN-EDGE-CRITICAL",
            "bandwidth percent 25",
        ],
        vec![
            "interface GigabitEthernet0/1",
            "service-policy output WAN-EDGE",
        ],
    ] {
        assert!(tree.find(&path).is_some(), "{path:?} is not in:\n{running}");
    }

    // And the policy is attached where the change will look for it afterwards.
    let shown = sim
        .exec(r1, "show policy-map interface GigabitEthernet0/1")
        .expect("show policy-map interface");
    assert!(shown.contains("Service-policy output: WAN-EDGE"), "{shown}");
    for class in [
        "WAN-EDGE-VOICE",
        "WAN-EDGE-VIDEO",
        "WAN-EDGE-CRITICAL",
        "class-default",
    ] {
        assert!(
            shown.contains(&format!("Class-map: {class} (match-any)")),
            "{class} is missing from:\n{shown}"
        );
    }
}

#[tokio::test]
async fn a_simulated_router_takes_the_shaped_policies_line_for_line() {
    let lab = lab(&["SW1"]).await;

    // The WAN policy inside a shaper.
    let (mut sim, r1) = router();
    let rendered = lines(
        &lab,
        "ios_qos_wan_edge",
        json!({"interface": "Gi0/1", "shape_kbps": 50000}),
    )
    .await;
    typed(&mut sim, r1, &rendered);
    let running = sim.running_config(r1).expect("running-config");
    let tree = ConfigTree::parse(&running, Syntax::Ios);
    for path in [
        vec![
            "policy-map WAN-EDGE-SHAPE",
            "class class-default",
            "shape average 50000000",
        ],
        vec![
            "policy-map WAN-EDGE-SHAPE",
            "class class-default",
            "service-policy WAN-EDGE",
        ],
        vec![
            "interface GigabitEthernet0/1",
            "service-policy output WAN-EDGE-SHAPE",
        ],
    ] {
        assert!(tree.find(&path).is_some(), "{path:?} is not in:\n{running}");
    }

    // The shaper on its own, on another router.
    let (mut sim, r1) = router();
    let rendered = lines(
        &lab,
        "ios_qos_shaped_uplink",
        json!({"interface": "Gi0/1", "rate_kbps": 50000}),
    )
    .await;
    typed(&mut sim, r1, &rendered);
    let running = sim.running_config(r1).expect("running-config");
    let tree = ConfigTree::parse(&running, Syntax::Ios);
    for path in [
        vec![
            "policy-map SHAPE-50000K",
            "class class-default",
            "shape average 50000000",
        ],
        vec![
            "interface GigabitEthernet0/1",
            "service-policy output SHAPE-50000K",
        ],
    ] {
        assert!(tree.find(&path).is_some(), "{path:?} is not in:\n{running}");
    }
    let shown = sim
        .exec(r1, "show policy-map interface GigabitEthernet0/1")
        .expect("show policy-map interface");
    assert!(
        shown.contains("Service-policy output: SHAPE-50000K"),
        "{shown}"
    );
}

#[tokio::test]
async fn a_simulated_router_takes_the_marking_policy_of_an_access_port() {
    // Only mode `set`: it is ordinary modular QoS. The simulator has no
    // Catalyst `mls qos` commands, so mode `trust` and the global `mls qos`
    // line rest on the two Catalyst guides alone.
    let lab = lab(&["SW1"]).await;
    let (mut sim, r1) = router();
    let rendered = lines(
        &lab,
        "ios_qos_access_port_dscp",
        json!({"interface": "Gi0/0", "mode": "set", "dscp": "af21"}),
    )
    .await;
    typed(&mut sim, r1, &rendered);
    let running = sim.running_config(r1).expect("running-config");
    let tree = ConfigTree::parse(&running, Syntax::Ios);
    for path in [
        vec![
            "policy-map MARK-DSCP-AF21",
            "class class-default",
            "set dscp af21",
        ],
        vec![
            "interface GigabitEthernet0/0",
            "service-policy input MARK-DSCP-AF21",
        ],
    ] {
        assert!(tree.find(&path).is_some(), "{path:?} is not in:\n{running}");
    }
}

/// Every line of a configuration with the section it sits in. This is what
/// the change pipeline compares after it has applied a change
/// (`nocturne-changes`, `compare::lines_of`); it is repeated here because a
/// feature crate does not link to another.
fn placed_lines(tree: &ConfigTree) -> BTreeSet<String> {
    fn walk(nodes: &[Node], prefix: &str, out: &mut BTreeSet<String>) {
        for node in nodes {
            let path = if prefix.is_empty() {
                node.text.trim().to_string()
            } else {
                format!("{prefix} / {}", node.text.trim())
            };
            out.insert(path.clone());
            walk(&node.children, &path, out);
        }
    }
    let mut lines = BTreeSet::new();
    walk(&tree.roots, "", &mut lines);
    lines
}

/// A change is verified by comparing the device with what the planner
/// predicted from the lines; a difference rolls the change back. So for every
/// IOS template, and for values a person would really type (numbers, names,
/// any order, any case), the configuration the simulated router ends up with
/// must be exactly the prediction.
#[tokio::test]
async fn a_simulated_router_ends_up_with_exactly_the_configuration_the_planner_predicts() {
    let lab = lab(&["SW1"]).await;
    let cases = [
        ("ios_qos_wan_edge", json!({"interface": "Gi0/1"})),
        (
            "ios_qos_wan_edge",
            json!({"interface": "Gi0/1", "shape_kbps": 50000}),
        ),
        (
            "ios_qos_wan_edge",
            json!({
                "interface": "gi 0/1", "policy_name": "BRANCH", "shape_kbps": 64,
                "voice_dscp": "EF, 44", "voice_percent": "33",
                "video_dscp": [38, "af41", "36", 32], "video_percent": 33,
                "critical_dscp": [8, 7, 6, 5, 4, 3, "default", "CS3"], "critical_percent": 33,
            }),
        ),
        (
            "ios_qos_wan_edge",
            json!({"interface": "Gi0/1", "policy_name": "A".repeat(31), "shape_kbps": 128_000_000}),
        ),
        (
            "ios_qos_shaped_uplink",
            json!({"interface": "Gi0/1", "rate_kbps": 8}),
        ),
        (
            "ios_qos_shaped_uplink",
            json!({"interface": "GigabitEthernet0/1", "rate_kbps": 128_000_000, "policy_name": "A".repeat(40)}),
        ),
        (
            "ios_qos_access_port_dscp",
            json!({"interface": "Gi0/0", "mode": "set", "dscp": "af21"}),
        ),
        (
            "ios_qos_access_port_dscp",
            json!({"interface": "Gi0/0", "mode": "SET", "dscp": 46}),
        ),
        (
            "ios_qos_access_port_dscp",
            json!({"interface": "Gi0/0", "mode": "set", "dscp": "0", "policy_name": "SCAVENGE"}),
        ),
        (
            "ios_qos_access_port_dscp",
            json!({"interface": "Gi0/0", "mode": "set", "dscp": 63}),
        ),
    ];
    for (id, vars) in cases {
        let rendered = lines(&lab, id, vars.clone()).await;
        let (mut sim, r1) = router();
        let before = sim.running_config(r1).expect("running-config");
        typed(&mut sim, r1, &rendered);
        let after = sim.running_config(r1).expect("running-config");

        let mut predicted = ConfigTree::parse(&before, Syntax::Ios);
        let report = predicted.apply(&rendered, Syntax::Ios);
        assert!(
            report.unmodelled.is_empty(),
            "{id} {vars}: the planner cannot model {:?}",
            report.unmodelled
        );
        let predicted = placed_lines(&predicted);
        let shown = placed_lines(&ConfigTree::parse(&after, Syntax::Ios));
        let missing: Vec<&String> = predicted.difference(&shown).collect();
        let unexpected: Vec<&String> = shown.difference(&predicted).collect();
        assert!(
            missing.is_empty() && unexpected.is_empty(),
            "{id} {vars}\n  predicted but not on the router: {missing:#?}\n  on the router but not predicted: {unexpected:#?}"
        );
        // And the change added something: an empty prediction would agree
        // with a router that took nothing.
        assert!(
            shown.len() > placed_lines(&ConfigTree::parse(&before, Syntax::Ios)).len(),
            "{id} {vars}: nothing was configured"
        );
    }
}

// --------------------------------------------------------------- the change

/// The planner of the change pipeline reads hand-written lines section by
/// section. What it predicts from the WAN policy must be the nested policy
/// the template means, not a flat list.
#[tokio::test]
async fn the_wan_policy_is_predicted_as_a_nested_policy_by_the_config_engine() {
    let lab = lab(&["SW1"]).await;
    let rendered = lines(
        &lab,
        "ios_qos_wan_edge",
        json!({"interface": "Gi0/1", "shape_kbps": 50000}),
    )
    .await;
    let mut tree = ConfigTree::parse("hostname EDGE1\n", Syntax::Ios);
    tree.apply(&rendered, Syntax::Ios);
    let predicted = tree.render();
    for path in [
        vec!["class-map match-any WAN-EDGE-VOICE", "match dscp ef"],
        vec![
            "policy-map WAN-EDGE",
            "class WAN-EDGE-VOICE",
            "priority percent 10",
        ],
        vec![
            "policy-map WAN-EDGE",
            "class WAN-EDGE-CRITICAL",
            "bandwidth percent 25",
        ],
        vec![
            "policy-map WAN-EDGE-SHAPE",
            "class class-default",
            "shape average 50000000",
        ],
        vec![
            "policy-map WAN-EDGE-SHAPE",
            "class class-default",
            "service-policy WAN-EDGE",
        ],
        vec![
            "interface GigabitEthernet0/1",
            "service-policy output WAN-EDGE-SHAPE",
        ],
    ] {
        assert!(
            tree.find(&path).is_some(),
            "{path:?} is not in:\n{predicted}"
        );
    }
}

/// A build can leave the change pipeline out. A template then has nowhere to
/// put its lines, and has to say so instead of pretending to have applied.
#[tokio::test]
async fn without_a_change_pipeline_a_qos_template_previews_and_says_it_cannot_apply() {
    // The harness of the other tests registers a stand-in `changes.create`;
    // this app has the core and this feature and nothing else.
    let mut builder = nocturne_app::testing::builder();
    nocturne_automation::init(&mut builder);
    let lab = nocturne_app::testing::lab(builder, &["SW1"]).await;
    let edge = lab
        .app
        .rpc(
            "devices.create",
            json!({"name": "EDGE1", "address": "192.0.2.1", "profile": "cisco_ios"}),
        )
        .await
        .expect("devices.create")["id"]
        .as_str()
        .expect("id")
        .to_string();
    let request = json!({"template_id": "ios_qos_shaped_uplink", "device_ids": [edge],
                         "vars": {"interface": "Gi0/1", "rate_kbps": 50000}});

    let err = lab
        .app
        .rpc("templates.apply", request.clone())
        .await
        .expect_err("there is no change pipeline to hand the lines to");
    assert_eq!(err.code, "unavailable", "{}", err.message);
    assert!(
        err.message.contains("no change pipeline"),
        "{}",
        err.message
    );

    let preview = lab
        .app
        .rpc("templates.render", request)
        .await
        .expect("templates.render");
    assert_eq!(
        preview[0]["lines"],
        json!([
            "policy-map SHAPE-50000K",
            " class class-default",
            "  shape average 50000000",
            "interface GigabitEthernet0/1",
            " service-policy output SHAPE-50000K",
        ])
    );
    assert!(
        lab.fake("SW1").seen().is_empty(),
        "nothing is sent to any device"
    );
}

#[tokio::test]
async fn applying_a_qos_template_makes_one_change_and_sends_nothing() {
    let lab = lab(&["SW1"]).await;
    // The built-ins are written for real device types; the lab's fake switch
    // is a test type, so the targets are inventory entries nothing answers for.
    let edge = lab
        .app
        .rpc(
            "devices.create",
            json!({"name": "EDGE1", "address": "192.0.2.1", "profile": "cisco_ios"}),
        )
        .await
        .expect("devices.create")["id"]
        .as_str()
        .expect("id")
        .to_string();
    let gateway = lab
        .app
        .rpc(
            "devices.create",
            json!({"name": "GW1", "address": "192.0.2.2", "profile": "mikrotik_routeros"}),
        )
        .await
        .expect("devices.create")["id"]
        .as_str()
        .expect("id")
        .to_string();

    let applied = lab
        .app
        .rpc(
            "templates.apply",
            json!({"template_id": "ios_qos_shaped_uplink", "device_ids": [edge],
                   "vars": {"interface": "Gi0/1", "rate_kbps": 50000}}),
        )
        .await
        .expect("templates.apply");
    assert_eq!(applied, json!({"change_id": "chg-1"}));
    let applied = lab
        .app
        .rpc(
            "templates.apply",
            json!({"template_id": "routeros_qos_simple_queue", "device_ids": [gateway],
                   "vars": required_only("routeros_qos_simple_queue")}),
        )
        .await
        .expect("templates.apply");
    assert_eq!(applied, json!({"change_id": "chg-2"}));

    // A template that refuses its variables leaves nothing behind.
    let err = lab
        .app
        .rpc(
            "templates.apply",
            json!({"template_id": "ios_qos_wan_edge", "device_ids": [edge],
                   "vars": {"interface": "Gi0/1", "voice_percent": 80, "video_percent": 30}}),
        )
        .await
        .expect_err("the shares are too many");
    assert_eq!(err.code, "invalid");
    assert!(
        err.message.contains("`EDGE1`") && err.message.contains("add up to 135 percent"),
        "{}",
        err.message
    );
    // And one written for another vendor is not rendered for this one.
    let err = lab
        .app
        .rpc(
            "templates.apply",
            json!({"template_id": "routeros_qos_queue_tree", "device_ids": [edge],
                   "vars": {"interface": "ether1", "rate_kbps": 20000}}),
        )
        .await
        .expect_err("RouterOS lines for an IOS device");
    assert!(err.message.contains("mikrotik_routeros"), "{}", err.message);

    let created = lab.app.state::<Changes>();
    let created = created.created.lock().expect("lock");
    assert_eq!(created.len(), 2, "the two that rendered, and no other");
    assert_eq!(created[0]["origin"], json!("template"));
    assert_eq!(
        created[0]["title"],
        json!("Shape an uplink to its circuit rate (IOS)")
    );
    assert_eq!(
        created[0]["targets"],
        json!([{
            "device_id": edge,
            "intent": {"type": "lines", "lines": [
                "policy-map SHAPE-50000K",
                " class class-default",
                "  shape average 50000000",
                "interface GigabitEthernet0/1",
                " service-policy output SHAPE-50000K",
            ]},
        }])
    );
    assert_eq!(
        created[1]["targets"][0]["intent"]["lines"],
        json!(["/queue simple add name=sub-1042 target=192.168.88.251/32 max-limit=5000k/15000k"])
    );
    assert!(
        lab.fake("SW1").seen().is_empty(),
        "nothing is sent to any device"
    );
}

// ------------------------------------------- what a second run finds there

#[tokio::test]
async fn a_queue_tree_adds_its_named_queues_before_any_firewall_rule_and_labels_each_rule() {
    // Queue names are unique on a router; mangle rules carry no name and are
    // added again on every run. Queues first means that a second run is
    // refused at its first line, before the firewall is touched, and the label
    // is how the rules this template added can be found again.
    let lab = lab(&["SW1"]).await;
    for (vars, prefix) in [
        (required_only("routeros_qos_queue_tree"), "wan"),
        (
            json!({"interface": "pppoe-out1", "rate_kbps": 7777, "name_prefix": "up"}),
            "up",
        ),
    ] {
        let rendered = lines(&lab, "routeros_qos_queue_tree", vars).await;
        let first_rule = rendered
            .iter()
            .position(|line| line.starts_with("/ip firewall mangle add"))
            .expect("mangle rules");
        let last_queue = rendered
            .iter()
            .rposition(|line| line.starts_with("/queue tree add"))
            .expect("queues");
        assert!(last_queue < first_rule, "{rendered:#?}");
        for rule in &rendered[first_rule..] {
            assert!(rule.starts_with("/ip firewall mangle add"), "{rule}");
            assert!(
                rule.ends_with(&format!(" comment={prefix}-qos")),
                "every rule says which template run it belongs to: {rule}"
            );
        }
    }
}

// ------------------------------------------------- interface names in full

#[tokio::test]
async fn an_abbreviation_the_router_spells_out_is_refused_rather_than_sent_as_typed() {
    // IOS takes any unambiguous start of an interface type and shows the full
    // name afterwards. A spelling `ifname` does not expand would be rendered as
    // typed, predicted as a section that never exists, and rolled back by
    // resetting the real port.
    let lab = lab(&["SW1"]).await;
    let cases: [(&str, &str, Value); 7] = [
        ("ios_qos_wan_edge", "interface", json!({})),
        (
            "ios_qos_shaped_uplink",
            "interface",
            json!({"rate_kbps": 50000}),
        ),
        ("ios_qos_access_port_dscp", "interface", json!({})),
        ("ios_access_port", "interface", json!({"vlan": 10})),
        ("ios_trunk_port", "interface", json!({"vlans": [30]})),
        ("ios_disable_port", "interface", json!({})),
        ("ios_ntp", "source", json!({"servers": ["192.0.2.1"]})),
    ];
    let mut wrong = Vec::new();
    for (id, variable, base) in &cases {
        for spelling in [
            "TenGig1/1/1",
            "Gigabit0/3",
            "GigE0/3",
            "Fast0/1",
            "FastEth0/1",
            "TwentyFiveGig1/0/1",
        ] {
            let mut vars = base.clone();
            vars[*variable] = json!(spelling);
            match turned_away(&lab, id, &vars).await {
                Ok(said) if said.contains("full name") && said.contains(spelling) => {}
                Ok(said) => wrong.push(format!("{id} {spelling}: {said}")),
                Err(what) => wrong.push(what),
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");

    // A type it knows is written the router's way, whatever its case.
    for (given, wanted) in [
        ("dialer1", "interface Dialer1"),
        (
            "TENGIGABITETHERNET1/1/1",
            "interface TenGigabitEthernet1/1/1",
        ),
        ("virtual-template1", "interface Virtual-Template1"),
    ] {
        let rendered = lines(
            &lab,
            "ios_qos_shaped_uplink",
            json!({"interface": given, "rate_kbps": 2000}),
        )
        .await;
        assert_eq!(rendered[3], wanted, "{given}");
    }
}

// ------------------------------------------------------ the subscriber queue

#[tokio::test]
async fn a_subscriber_queue_can_be_put_before_a_queue_that_would_take_its_traffic() {
    // Simple queues are tried in order and a packet stops at the first that
    // matches it: a /32 added after a /24 that covers it never sees a packet.
    let lab = lab(&["SW1"]).await;
    let queue = |extra: Value| {
        let mut vars = required_only("routeros_qos_simple_queue");
        vars.as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("object").clone());
        vars
    };
    assert_eq!(
        lines(
            &lab,
            "routeros_qos_simple_queue",
            queue(json!({"place_before": "subnet-88"}))
        )
        .await,
        vec![
            "/queue simple add name=sub-1042 target=192.168.88.251/32 max-limit=5000k/15000k place-before=subnet-88"
        ]
    );
    for place_before in [
        json!("subnet 88"),
        json!("subnet-88;/system reboot"),
        json!("[find]"),
        json!("sub-1042"),
        json!("x".repeat(65)),
    ] {
        let said = turned_away(
            &lab,
            "routeros_qos_simple_queue",
            &queue(json!({"place_before": place_before})),
        )
        .await
        .unwrap_or_else(|what| panic!("{what}"));
        assert!(said.contains("Before the queue"), "{place_before}: {said}");
    }

    let template = lab
        .app
        .rpc("templates.get", json!({"id": "routeros_qos_simple_queue"}))
        .await
        .expect("templates.get");
    let description = template["description"].as_str().expect("description");
    assert!(
        description.contains("strict order") && description.contains("place-before"),
        "the description says why the order matters: {description}"
    );
}

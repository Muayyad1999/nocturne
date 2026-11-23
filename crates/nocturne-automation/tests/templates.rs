//! Templates: rendering, variable validation, the built-ins, and the fact
//! that a configuration template produces a change and never a command.

mod common;

use common::{Changes, lab};
use nocturne_app::{collect, inventory};
use nocturne_drivers::model::Kind;
use serde_json::{Value, json};

async fn render_one(lab: &nocturne_app::testing::Lab, body: &str, vars: Value) -> Value {
    lab.app
        .rpc(
            "templates.render",
            json!({"body": body, "device_ids": [lab.id("SW1")], "vars": vars}),
        )
        .await
        .expect("templates.render")[0]
        .clone()
}

#[tokio::test]
async fn a_name_the_template_does_not_have_is_an_error_that_says_which_name() {
    let lab = lab(&["SW1"]).await;
    let rendered = render_one(&lab, "ntp server {{ server }}", json!({})).await;
    assert_eq!(rendered["ok"], json!(false));
    let error = rendered["error"].as_str().expect("an error");
    assert!(error.contains("`server`"), "it names the variable: {error}");
    assert!(
        rendered["lines"].is_null(),
        "and renders nothing: {rendered:?}"
    );

    // With the variable, it renders — and never as an empty string.
    let rendered = render_one(
        &lab,
        "ntp server {{ server }}",
        json!({"server": "192.0.2.1"}),
    )
    .await;
    assert_eq!(rendered["lines"], json!(["ntp server 192.0.2.1"]));
}

#[tokio::test]
async fn a_template_that_needs_collected_data_says_to_collect_it_first() {
    let lab = lab(&["SW1"]).await;
    let body =
        "{% for neighbor in neighbors %}interface {{ neighbor.local_interface }}\n{% endfor %}";
    let rendered = render_one(&lab, body, json!({})).await;
    assert_eq!(rendered["ok"], json!(false));
    let error = rendered["error"].as_str().expect("an error");
    assert!(
        error.contains("collect neighbors for `SW1` first"),
        "{error}"
    );

    // Once neighbours are there, the same body renders.
    collect::store(
        &lab.app,
        &lab.id("SW1"),
        Kind::Neighbors,
        vec![json!({
            "local_interface": "GigabitEthernet0/1",
            "device": "CORE1",
            "remote_interface": "Gi1/0/1",
            "platform": "", "address": "10.0.0.1", "protocol": "cdp"
        })],
        "test",
    )
    .expect("neighbours are stored");
    let rendered = render_one(&lab, body, json!({})).await;
    assert_eq!(rendered["lines"], json!(["interface GigabitEthernet0/1"]));
}

#[tokio::test]
async fn the_render_context_carries_the_device_and_what_was_collected_from_it() {
    let lab = lab(&["SW1"]).await;
    collect::store(
        &lab.app,
        &lab.id("SW1"),
        Kind::Facts,
        vec![json!({"hostname": "SW1", "model": "WS-C2960", "os_version": "15.2(4)E10"})],
        "test",
    )
    .expect("facts");
    collect::store(
        &lab.app,
        &lab.id("SW1"),
        Kind::Vlans,
        vec![json!({"id": 10, "name": "users", "status": "active"})],
        "test",
    )
    .expect("vlans");

    let rendered = render_one(
        &lab,
        "! {{ device.name }} {{ device.site }} {{ facts.model }} {{ vlans | length }}",
        json!({}),
    )
    .await;
    assert_eq!(rendered["lines"], json!(["! SW1 Lab WS-C2960 1"]));
}

#[tokio::test]
async fn a_variable_is_checked_against_its_kind_with_its_label_in_the_message() {
    let lab = lab(&["SW1"]).await;
    let variables = json!([
        {"name": "vlan", "label": "Access VLAN", "kind": "vlan", "required": true},
        {"name": "address", "label": "Address", "kind": "ip", "required": true},
        {"name": "prefix", "label": "Prefix", "kind": "cidr", "required": true},
        {"name": "count", "label": "Count", "kind": "int", "required": true},
        {"name": "on", "label": "Enabled", "kind": "bool", "required": true},
        {"name": "port", "label": "Port", "kind": "interface", "required": true},
        {"name": "servers", "label": "Servers", "kind": "list", "required": true},
        {"name": "level", "label": "Level", "kind": "choice", "required": true,
         "choices": ["low", "high"]},
    ]);
    let good = json!({
        "vlan": 10, "address": "10.0.0.1", "prefix": "10.0.0.0/24", "count": "7",
        "on": "yes", "port": "Gi0/1", "servers": "1.1.1.1, 8.8.8.8", "level": "high"
    });
    let rendered = lab
        .app
        .rpc(
            "templates.render",
            json!({
                "body": "! {{ vlan }} {{ address }} {{ prefix }} {{ count }} {{ on }} \
                         {{ port }} {{ servers | join('+') }} {{ level }}",
                "variables": variables,
                "device_ids": [lab.id("SW1")],
                "vars": good,
            }),
        )
        .await
        .expect("templates.render")[0]
        .clone();
    assert_eq!(
        rendered["lines"],
        json!(["! 10 10.0.0.1 10.0.0.0/24 7 True Gi0/1 1.1.1.1+8.8.8.8 high"]),
        "a comma-separated string becomes a list and `yes` becomes a boolean"
    );

    for (field, value, wanted) in [
        ("vlan", json!(5000), "VLAN id between 1 and 4094"),
        ("vlan", json!(0), "VLAN id between 1 and 4094"),
        ("address", json!("10.0.0.300"), "must be an IP address"),
        ("prefix", json!("10.0.0.0"), "prefix length"),
        ("prefix", json!("10.0.0.0/40"), "prefix length"),
        // Digits only: `+24` is a number to a parser and not to a device.
        ("prefix", json!("10.0.0.0/+24"), "prefix length"),
        ("prefix", json!("10.0.0.0/24 secondary"), "prefix length"),
        ("count", json!("many"), "whole number"),
        ("on", json!("perhaps"), "true or false"),
        (
            "port",
            json!("Gi0/1\n shutdown"),
            "one interface name, on one line",
        ),
        ("level", json!("medium"), "must be one of low, high"),
    ] {
        let mut vars = good.clone();
        vars[field] = value.clone();
        let err = lab
            .app
            .rpc(
                "templates.render",
                json!({"body": "!", "variables": variables,
                       "device_ids": [lab.id("SW1")], "vars": vars}),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, "invalid", "{field}={value}");
        assert!(
            err.message.contains(wanted),
            "{field}={value}: {}",
            err.message
        );
    }

    // A required variable that is simply missing says so by its label.
    let err = lab
        .app
        .rpc(
            "templates.render",
            json!({"body": "!", "variables": variables,
                   "device_ids": [lab.id("SW1")], "vars": {}}),
        )
        .await
        .unwrap_err();
    assert!(
        err.message.contains("Access VLAN") && err.message.contains("required"),
        "{}",
        err.message
    );
}

#[tokio::test]
async fn validate_reports_what_a_body_uses_and_what_it_has_not_declared() {
    let lab = lab(&["SW1"]).await;
    let checked = lab
        .app
        .rpc(
            "templates.validate",
            json!({
                "body": "ntp server {{ server }}\n! {{ device.name }} {{ facts.model }}",
                "variables": [{"name": "server", "kind": "ip", "required": true}],
            }),
        )
        .await
        .expect("templates.validate");
    assert_eq!(checked["ok"], json!(true));
    assert_eq!(
        checked["used_variables"],
        json!(["device", "facts", "server"])
    );
    assert!(checked["undeclared"].as_array().expect("array").is_empty());

    let checked = lab
        .app
        .rpc(
            "templates.validate",
            json!({"body": "ntp server {{ server }}", "variables": []}),
        )
        .await
        .expect("templates.validate");
    assert_eq!(checked["ok"], json!(false));
    assert_eq!(checked["undeclared"], json!(["server"]));

    let checked = lab
        .app
        .rpc(
            "templates.validate",
            json!({"body": "{% for x in %}", "variables": []}),
        )
        .await
        .expect("templates.validate");
    assert_eq!(checked["ok"], json!(false));
    assert!(
        checked["problems"][0]["line"].as_u64().is_some(),
        "a syntax error says where: {checked:?}"
    );
}

#[tokio::test]
async fn a_template_can_refuse_its_variables_in_its_own_words() {
    let lab = lab(&["SW1"]).await;
    let body = "{% if low > high %}{{ fail('Lowest VLAN (' ~ low ~ ') is above Highest VLAN (' ~ high ~ ')') }}{% endif %}\n\
                vlan {{ low }}-{{ high }}";
    let rendered = render_one(&lab, body, json!({"low": 30, "high": 20})).await;
    assert_eq!(rendered["ok"], json!(false));
    assert_eq!(
        rendered["error"],
        json!("Lowest VLAN (30) is above Highest VLAN (20)"),
        "the template's message, as written"
    );
    assert!(rendered["lines"].is_null(), "and no line: {rendered:?}");

    let rendered = render_one(&lab, body, json!({"low": 20, "high": 30})).await;
    assert_eq!(rendered["lines"], json!(["vlan 20-30"]));

    // A refusal stops an apply before a change exists.
    lab.app
        .rpc(
            "templates.save",
            json!({"id": "vlan_range", "name": "VLAN range", "kind": "config", "body": body,
                   "variables": [{"name": "low", "kind": "vlan", "required": true},
                                 {"name": "high", "kind": "vlan", "required": true}]}),
        )
        .await
        .expect("a template that calls `fail` can be saved");
    let err = lab
        .app
        .rpc(
            "templates.apply",
            json!({"template_id": "vlan_range", "device_ids": [lab.id("SW1")],
                   "vars": {"low": 30, "high": 20}}),
        )
        .await
        .expect_err("the template refuses");
    assert_eq!(err.code, "invalid");
    assert_eq!(
        err.message,
        "`SW1`: Lowest VLAN (30) is above Highest VLAN (20)"
    );
    assert!(
        lab.app
            .state::<Changes>()
            .created
            .lock()
            .expect("lock")
            .is_empty(),
        "a refused template creates no change"
    );
}

#[tokio::test]
async fn a_refusal_is_one_line_of_bounded_length_whatever_the_template_put_in_it() {
    // A template may quote the value it refuses, and a value is whatever the
    // caller sent: a megabyte, a line break, an escape sequence. The refusal
    // goes to a screen, an error and a log line, once per device.
    let lab = lab(&["SW1", "SW2"]).await;
    let body = "{{ fail('Name: `' ~ name ~ '` is not a name') }}";
    let hostile = format!("x\n\u{1b}[2J\r{}", "A".repeat(1_000_000));
    let rendered = lab
        .app
        .rpc(
            "templates.render",
            json!({"body": body, "device_ids": [lab.id("SW1"), lab.id("SW2")],
                   "vars": {"name": hostile}}),
        )
        .await
        .expect("templates.render");
    for device in rendered.as_array().expect("one per device") {
        assert_eq!(device["ok"], json!(false));
        let error = device["error"].as_str().expect("an error");
        assert!(
            error.starts_with("Name: `x  [2J AAAA"),
            "the start is kept, with a space where a control character was: {:?}",
            &error[..40]
        );
        assert!(!error.contains(char::is_control), "{:?}", &error[..40]);
        assert!(error.ends_with('…'), "and it says that it was cut");
        assert_eq!(error.chars().count(), 401);
    }

    // A message that fits is passed on exactly as written.
    let rendered = render_one(&lab, "{{ fail('Größe: 10 % too many') }}", json!({})).await;
    assert_eq!(rendered["error"], json!("Größe: 10 % too many"));

    // The same through `templates.apply`, whose error carries the message.
    lab.app
        .rpc(
            "templates.save",
            json!({"id": "named", "name": "Named", "kind": "config", "body": body,
                   "variables": [{"name": "name", "kind": "string", "required": true}]}),
        )
        .await
        .expect("templates.save");
    let err = lab
        .app
        .rpc(
            "templates.apply",
            json!({"template_id": "named", "device_ids": [lab.id("SW1")],
                   "vars": {"name": hostile}}),
        )
        .await
        .expect_err("the template refuses");
    assert_eq!(err.code, "invalid");
    assert!(err.message.chars().count() < 420, "{}", err.message.len());
    assert!(!err.message.contains(char::is_control));
}

#[tokio::test]
async fn what_the_template_language_provides_is_not_a_missing_variable() {
    // `fail`, `range` and `namespace` are functions of the language. The
    // analysis that finds the names a body reads lists them too, and they
    // must be neither reported as undeclared nor refused as unknown.
    let lab = lab(&["SW1"]).await;
    let body = "{% if count > 4 %}{{ fail('too many') }}{% endif %}\n\
                {% for n in range(count) %}vlan {{ first + n }}\n{% endfor %}";
    let checked = lab
        .app
        .rpc(
            "templates.validate",
            json!({"body": body,
                   "variables": [{"name": "count", "kind": "int", "required": true},
                                 {"name": "first", "kind": "vlan", "required": true}]}),
        )
        .await
        .expect("templates.validate");
    assert_eq!(checked["ok"], json!(true), "{checked}");
    assert_eq!(checked["used_variables"], json!(["count", "first"]));
    assert_eq!(checked["undeclared"], json!([]));

    let rendered = render_one(&lab, body, json!({"count": 3, "first": 10})).await;
    assert_eq!(rendered["lines"], json!(["vlan 10", "vlan 11", "vlan 12"]));

    // A name that really is missing is still said to be.
    let rendered = render_one(&lab, "{{ fail(reason) }}", json!({})).await;
    assert!(
        rendered["error"]
            .as_str()
            .expect("an error")
            .contains("`reason`"),
        "{rendered:?}"
    );
}

#[tokio::test]
async fn every_built_in_template_renders_to_the_lines_it_is_meant_to() {
    let lab = lab(&["SW1"]).await;
    collect::store(
        &lab.app,
        &lab.id("SW1"),
        Kind::Neighbors,
        vec![json!({
            "local_interface": "GigabitEthernet0/1", "device": "CORE1",
            "remote_interface": "Gi1/0/1", "platform": "", "address": "10.0.0.1",
            "protocol": "cdp"
        })],
        "test",
    )
    .expect("neighbours");

    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "ios_ntp",
            json!({"servers": ["192.0.2.1", "192.0.2.2"]}),
            vec!["ntp server 192.0.2.1", "ntp server 192.0.2.2"],
        ),
        (
            "ios_syslog",
            json!({"hosts": ["10.0.0.5"]}),
            vec!["logging host 10.0.0.5", "logging trap informational"],
        ),
        (
            "ios_banner",
            json!({"text": "Authorised use only"}),
            vec!["banner login ^C", "Authorised use only", "^C"],
        ),
        (
            "ios_vlan",
            json!({"vlan": 30, "vlan_name": "guests"}),
            vec!["vlan 30", " name guests"],
        ),
        (
            "ios_access_port",
            json!({"interface": "Gi0/5", "vlan": 10, "description": "desk 5"}),
            vec![
                "interface GigabitEthernet0/5",
                " description desk 5",
                " switchport mode access",
                " switchport access vlan 10",
                " spanning-tree portfast",
            ],
        ),
        (
            "ios_access_port",
            json!({"interface": "Gi0/5", "vlan": 10, "voice_vlan": 110}),
            vec![
                "interface GigabitEthernet0/5",
                " switchport mode access",
                " switchport access vlan 10",
                " switchport voice vlan 110",
                " spanning-tree portfast",
            ],
        ),
        (
            "ios_trunk_port",
            json!({"interface": "Gi0/1", "vlans": [30, 31], "native_vlan": 999}),
            vec![
                "interface GigabitEthernet0/1",
                " switchport mode trunk",
                " switchport trunk native vlan 999",
                " switchport trunk allowed vlan add 30,31",
            ],
        ),
        (
            "ios_port_description_from_neighbor",
            json!({}),
            vec![
                "interface GigabitEthernet0/1",
                " description to CORE1 Gi1/0/1",
            ],
        ),
        (
            "ios_dns",
            json!({"servers": ["1.1.1.1"], "domain": "lab.example"}),
            vec!["ip name-server 1.1.1.1", "ip domain-name lab.example"],
        ),
        (
            "ios_disable_port",
            json!({"interface": "Gi0/9"}),
            vec![
                "interface GigabitEthernet0/9",
                " description DISABLED - unused",
                " shutdown",
            ],
        ),
        (
            "ios_mgmt_acl_vty",
            json!({"prefixes": ["10.0.0.0/24", "192.168.1.0/28"]}),
            vec![
                "ip access-list standard MGMT-VTY",
                " permit 10.0.0.0 0.0.0.255",
                " permit 192.168.1.0 0.0.0.15",
                "line vty 0 15",
                " access-class MGMT-VTY in",
                " transport input ssh",
            ],
        ),
        (
            "ios_health_check",
            json!({}),
            vec![
                "show version",
                "show ip interface brief",
                "show cdp neighbors",
                "show logging",
            ],
        ),
        (
            "routeros_ntp_client",
            json!({"servers": ["192.0.2.1", "192.0.2.2"]}),
            vec!["/system ntp client set enabled=yes servers=192.0.2.1,192.0.2.2"],
        ),
        (
            "routeros_syslog",
            json!({"host": "10.0.0.5"}),
            vec![
                "/system logging action add name=nocturne-remote target=remote remote=10.0.0.5 remote-port=514",
                "/system logging add action=nocturne-remote topics=info",
            ],
        ),
        (
            "routeros_vlan_bridge",
            json!({"vlan": 30}),
            vec!["/interface vlan add name=vlan30 vlan-id=30 interface=bridge"],
        ),
        (
            "routeros_lan_with_dhcp",
            json!({
                "interface": "vlan30", "address": "10.30.0.1/24", "network": "10.30.0.0/24",
                "gateway": "10.30.0.1", "pool_from": "10.30.0.100", "pool_to": "10.30.0.200",
                "dns": ["1.1.1.1", "9.9.9.9"]
            }),
            vec![
                "/ip address add address=10.30.0.1/24 interface=vlan30",
                "/ip pool add name=pool-vlan30 ranges=10.30.0.100-10.30.0.200",
                "/ip dhcp-server network add address=10.30.0.0/24 gateway=10.30.0.1 dns-server=1.1.1.1,9.9.9.9",
                "/ip dhcp-server add name=dhcp-vlan30 interface=vlan30 address-pool=pool-vlan30 lease-time=1d disabled=no",
            ],
        ),
        (
            "routeros_mgmt_services",
            json!({"prefixes": ["10.0.0.0/24"]}),
            vec![
                "/ip service set ssh address=10.0.0.0/24",
                "/ip service set winbox address=10.0.0.0/24",
                "/ip service set api address=10.0.0.0/24",
                "/ip service set telnet disabled=yes",
                "/ip service set ftp disabled=yes",
                "/ip service set www disabled=yes",
            ],
        ),
    ];

    for (id, vars, expected) in &cases {
        let rendered = lab
            .app
            .rpc(
                "templates.render",
                json!({"template_id": id, "device_ids": [lab.id("SW1")], "vars": vars}),
            )
            .await
            .unwrap_or_else(|err| panic!("{id}: {err}"))[0]
            .clone();
        assert_eq!(rendered["ok"], json!(true), "{id}: {:?}", rendered["error"]);
        assert_eq!(
            rendered["lines"],
            json!(expected),
            "{id} rendered the wrong lines"
        );
    }

    // Every built-in was covered by the table above. The ones tagged `qos`
    // have a table of their own, in `qos_templates.rs`, which checks in turn
    // that it covers every one carrying the tag.
    let listed: Vec<String> = lab
        .app
        .rpc("templates.list", json!({}))
        .await
        .expect("templates.list")
        .as_array()
        .expect("array")
        .iter()
        .filter(|template| template["builtin"] == json!(true))
        .filter(|template| template["tags"] != json!(["qos"]))
        .map(|template| template["id"].as_str().expect("id").to_string())
        .collect();
    for id in &listed {
        assert!(
            cases.iter().any(|(covered, ..)| covered == id),
            "the built-in `{id}` has no render test"
        );
    }
    assert_eq!(listed.len(), 16, "the built-ins: {listed:?}");
}

#[tokio::test]
async fn no_built_in_template_asks_for_a_secret() {
    // A rendered line goes into a change document, into job output and onto a
    // screen. Secrets live in the vault; a built-in that took a password would
    // put one in all three.
    let lab = lab(&["SW1"]).await;
    let listed = lab
        .app
        .rpc("templates.list", json!({}))
        .await
        .expect("templates.list");
    for template in listed.as_array().expect("array") {
        if template["builtin"] != json!(true) {
            continue;
        }
        let id = template["id"].as_str().expect("id");
        let haystack = format!(
            "{} {}",
            template["body"].as_str().unwrap_or_default().to_lowercase(),
            serde_json::to_string(&template["variables"])
                .unwrap_or_default()
                .to_lowercase()
        );
        for forbidden in [
            "password",
            "secret",
            "community",
            "passphrase",
            "private-key",
        ] {
            assert!(
                !haystack.contains(forbidden),
                "the built-in `{id}` mentions `{forbidden}`"
            );
        }
    }
}

#[tokio::test]
async fn a_configuration_template_becomes_a_change_and_sends_nothing() {
    let lab = lab(&["SW1", "SW2"]).await;
    lab.app
        .rpc(
            "templates.save",
            json!({
                "id": "local_ntp", "name": "NTP", "kind": "config",
                "body": "ntp server {{ server }}\n! for {{ device.name }}",
                "variables": [{"name": "server", "kind": "ip", "required": true}],
            }),
        )
        .await
        .expect("templates.save");

    let applied = lab
        .app
        .rpc(
            "templates.apply",
            json!({"template_id": "local_ntp", "filter": {}, "vars": {"server": "192.0.2.1"},
                   "title": "NTP everywhere"}),
        )
        .await
        .expect("templates.apply");
    assert_eq!(applied["change_id"], json!("chg-1"));

    let created = lab.app.state::<Changes>();
    let created = created.created.lock().expect("lock");
    assert_eq!(created.len(), 1, "one change for the whole run");
    let change = &created[0];
    assert_eq!(change["title"], json!("NTP everywhere"));
    assert_eq!(change["origin"], json!("template"));
    let targets = change["targets"].as_array().expect("targets");
    assert_eq!(targets.len(), 2, "one target per device");
    assert_eq!(targets[0]["intent"]["type"], json!("lines"));
    assert_eq!(
        targets[0]["intent"]["lines"],
        json!(["ntp server 192.0.2.1", "! for SW1"])
    );
    assert_eq!(
        targets[1]["intent"]["lines"],
        json!(["ntp server 192.0.2.1", "! for SW2"]),
        "each device gets its own rendering"
    );

    for name in ["SW1", "SW2"] {
        assert!(
            lab.fake(name).seen().is_empty(),
            "{name} was sent configuration directly"
        );
        assert!(!lab.fake(name).running_config().contains("ntp server"));
    }
}

#[tokio::test]
async fn a_dry_run_of_a_configuration_template_creates_no_change() {
    let lab = lab(&["SW1"]).await;
    lab.app
        .rpc(
            "templates.save",
            json!({"id": "local_ntp", "name": "NTP", "kind": "config",
                   "body": "ntp server {{ server }}",
                   "variables": [{"name": "server", "kind": "ip", "required": true}]}),
        )
        .await
        .expect("templates.save");
    let previewed = lab
        .app
        .rpc(
            "templates.apply",
            json!({"template_id": "local_ntp", "filter": {},
                   "vars": {"server": "192.0.2.1"}, "dry_run": true}),
        )
        .await
        .expect("templates.apply");
    assert_eq!(previewed["dry_run"], json!(true));
    assert_eq!(
        previewed["rendered"][0]["lines"],
        json!(["ntp server 192.0.2.1"])
    );
    assert!(
        lab.app
            .state::<Changes>()
            .created
            .lock()
            .expect("lock")
            .is_empty(),
        "a dry run creates nothing"
    );
}

#[tokio::test]
async fn a_commands_template_becomes_a_run_of_read_only_commands() {
    let lab = lab(&["SW1"]).await;
    // The built-in health check is written for `cisco_ios`; this device type is
    // the test one, so the same body is saved without a platform restriction.
    lab.app
        .rpc(
            "templates.save",
            json!({"id": "health", "name": "Health", "kind": "commands",
                   "body": "show version\nshow ip interface brief\nshow cdp neighbors\nshow logging"}),
        )
        .await
        .expect("templates.save");
    let started = lab
        .app
        .rpc(
            "templates.apply",
            json!({"template_id": "health", "device_ids": [lab.id("SW1")]}),
        )
        .await
        .expect("templates.apply");
    let run_id = started["run_id"].as_str().expect("a run id").to_string();
    common::finished(&lab.app, &started).await;
    let results = common::results(&lab.app, &run_id).await;
    assert_eq!(results.len(), 1);
    let commands: Vec<&str> = results[0]["outputs"]
        .as_array()
        .expect("outputs")
        .iter()
        .map(|output| output["command"].as_str().expect("a command"))
        .collect();
    assert_eq!(
        commands,
        vec![
            "show version",
            "show ip interface brief",
            "show cdp neighbors",
            "show logging"
        ]
    );
    assert!(
        lab.app
            .state::<Changes>()
            .created
            .lock()
            .expect("lock")
            .is_empty(),
        "a commands template creates no change"
    );
}

#[tokio::test]
async fn a_commands_template_that_renders_a_change_is_refused_before_anything_is_sent() {
    let lab = lab(&["SW1"]).await;
    lab.app
        .rpc(
            "templates.save",
            json!({"id": "sneaky", "name": "Sneaky", "kind": "commands",
                   "body": "show version\nreload in {{ minutes }}",
                   "variables": [{"name": "minutes", "kind": "int", "required": true}]}),
        )
        .await
        .expect("templates.save");
    let err = lab
        .app
        .rpc(
            "templates.apply",
            json!({"template_id": "sneaky", "device_ids": [lab.id("SW1")],
                   "vars": {"minutes": 5}}),
        )
        .await
        .expect_err("a command template cannot smuggle a change through");
    assert_eq!(err.code, "forbidden");
    assert!(err.message.contains("reload in 5"), "{}", err.message);
    assert!(lab.fake("SW1").seen().is_empty());
}

#[tokio::test]
async fn a_template_written_for_another_platform_is_refused_with_the_reason() {
    let lab = lab(&["SW1"]).await;
    let err = lab
        .app
        .rpc(
            "templates.apply",
            json!({"template_id": "ios_ntp", "device_ids": [lab.id("SW1")],
                   "vars": {"servers": ["192.0.2.1"]}}),
        )
        .await
        .expect_err("SW1 is a test_ios device, not cisco_ios");
    assert_eq!(err.code, "invalid");
    assert!(
        err.message.contains("cisco_ios") && err.message.contains("SW1"),
        "{}",
        err.message
    );
}

#[tokio::test]
async fn built_ins_are_read_only_and_duplicating_one_makes_an_editable_copy() {
    let lab = lab(&["SW1"]).await;
    let err = lab
        .app
        .rpc(
            "templates.save",
            json!({"id": "ios_ntp", "name": "Mine", "body": "ntp server 1.1.1.1"}),
        )
        .await
        .expect_err("built-ins are read-only");
    assert_eq!(err.code, "forbidden");
    assert!(err.message.contains("duplicate it"), "{}", err.message);
    assert_eq!(
        lab.app
            .rpc("templates.delete", json!({"id": "ios_ntp"}))
            .await
            .expect_err("built-ins are read-only")
            .code,
        "forbidden"
    );

    let copy = lab
        .app
        .rpc("templates.duplicate", json!({"id": "ios_ntp"}))
        .await
        .expect("templates.duplicate");
    assert_eq!(copy["builtin"], json!(false));
    assert_eq!(copy["name"], json!("NTP servers (IOS) (copy)"));
    assert_eq!(copy["variables"], {
        let original = lab
            .app
            .rpc("templates.get", json!({"id": "ios_ntp"}))
            .await
            .expect("templates.get");
        original["variables"].clone()
    });
    // And the copy can be edited.
    let edited = lab
        .app
        .rpc(
            "templates.save",
            json!({"id": copy["id"], "name": "My NTP", "kind": "config",
                   "body": "ntp server {{ server }}",
                   "variables": [{"name": "server", "kind": "ip", "required": true}]}),
        )
        .await
        .expect("the copy is editable");
    assert_eq!(edited["name"], json!("My NTP"));
    assert_eq!(
        lab.app
            .rpc("templates.delete", json!({"id": copy["id"]}))
            .await
            .expect("deleted")["deleted"],
        json!(true)
    );
}

#[tokio::test]
async fn a_template_that_does_not_compile_is_not_saved() {
    let lab = lab(&["SW1"]).await;
    let err = lab
        .app
        .rpc(
            "templates.save",
            json!({"name": "Broken", "body": "{% if %}"}),
        )
        .await
        .expect_err("it does not compile");
    assert_eq!(err.code, "invalid");
    assert!(
        lab.app
            .rpc("templates.list", json!({}))
            .await
            .expect("list")
            .as_array()
            .expect("array")
            .iter()
            .all(|template| template["builtin"] == json!(true)),
        "nothing was stored"
    );

    let err = lab
        .app
        .rpc(
            "templates.save",
            json!({"name": "Choiceless", "body": "! {{ level }}",
                   "variables": [{"name": "level", "kind": "choice"}]}),
        )
        .await
        .expect_err("a choice with no choices");
    assert!(err.message.contains("lists no choices"), "{}", err.message);
}

// ------------------------------------------------ what one rendering may do

fn created(lab: &nocturne_app::testing::Lab) -> usize {
    lab.app
        .state::<Changes>()
        .created
        .lock()
        .expect("lock")
        .len()
}

#[tokio::test]
async fn a_rendering_longer_than_any_device_configuration_stops_at_one_mebibyte() {
    let lab = lab(&["SW1"]).await;
    // Five thousand interfaces with a thousand-character description: 5 MB.
    let body =
        "{% for i in range(5000) %}interface Gi0/{{ i }}\n description {{ d }}\n{% endfor %}";
    let rendered = render_one(&lab, body, json!({"d": "x".repeat(1000)})).await;
    assert_eq!(rendered["ok"], json!(false), "{}", rendered["error"]);
    assert!(rendered["lines"].is_null(), "nothing of it is kept");
    let error = rendered["error"].as_str().expect("an error");
    assert!(error.contains("more than 1 MiB"), "{error}");

    lab.app
        .rpc(
            "templates.save",
            json!({"id": "long", "name": "Long", "kind": "config", "body": body,
                   "variables": [{"name": "d", "kind": "string", "required": true}]}),
        )
        .await
        .expect("templates.save");
    let err = lab
        .app
        .rpc(
            "templates.apply",
            json!({"template_id": "long", "device_ids": [lab.id("SW1")],
                   "vars": {"d": "x".repeat(1000)}}),
        )
        .await
        .expect_err("too long to be a change");
    assert_eq!(err.code, "invalid");
    assert!(err.message.contains("more than 1 MiB"), "{}", err.message);
    assert_eq!(created(&lab), 0, "no change was made of it");
}

#[tokio::test]
async fn sixteen_mebibytes_is_the_most_one_call_renders_for_all_its_devices_together() {
    let names: Vec<String> = (1..=20).map(|n| format!("SW{n}")).collect();
    let lab = lab(&names.iter().map(String::as_str).collect::<Vec<_>>()).await;
    // 900 kB a device is within one device's share; twenty of them are not
    // within one call's.
    let rendered = lab
        .app
        .rpc(
            "templates.render",
            json!({"body": "{{ d }}", "filter": {}, "vars": {"d": "x".repeat(900_000)}}),
        )
        .await
        .expect("templates.render");
    let rendered = rendered.as_array().expect("one per device");
    assert_eq!(rendered.len(), 20, "every device has an answer");
    let ok = rendered
        .iter()
        .filter(|item| item["ok"] == json!(true))
        .count();
    assert_eq!(ok, 18, "18 x 900 kB fit in 16 MiB, a 19th does not");
    for refused in rendered.iter().filter(|item| item["ok"] == json!(false)) {
        let error = refused["error"].as_str().expect("an error");
        assert!(error.contains("16 MiB"), "{error}");
        assert!(refused["lines"].is_null());
    }
}

#[tokio::test]
async fn a_template_that_keeps_working_without_end_is_stopped_and_says_so() {
    let lab = lab(&["SW1"]).await;
    // Nine million turns of a loop that writes nothing.
    let body = "{% for i in range(3000) %}{% for j in range(3000) %}{% endfor %}{% endfor %}";
    let rendered = render_one(&lab, body, json!({})).await;
    assert_eq!(rendered["ok"], json!(false), "{rendered}");
    let error = rendered["error"].as_str().expect("an error");
    assert!(error.contains("steps of work"), "{error}");
}

// ----------------------------------------------- one value, one line, one word

/// What is said when a value is turned away: before rendering (the call
/// fails) or by the template (the device's `error`). `Err` is what happened
/// instead: a rendered line above all.
async fn turned_away(
    lab: &nocturne_app::testing::Lab,
    id: &str,
    vars: &Value,
) -> Result<String, String> {
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
            "{id} {vars} failed with `{}`: {}",
            err.code, err.message
        )),
        Ok(rendered) => match rendered[0]["error"].as_str() {
            Some(said) if rendered[0]["ok"] == json!(false) => Ok(said.to_string()),
            _ => Err(format!("{id} {vars} rendered to {}", rendered[0]["lines"])),
        },
    }
}

#[tokio::test]
async fn a_line_break_in_a_one_line_value_is_refused_and_a_banner_may_have_them() {
    let lab = lab(&["SW1"]).await;
    for (id, vars, label) in [
        (
            "ios_vlan",
            json!({"vlan": 30, "vlan_name": "USERS\ninterface GigabitEthernet0/1\n shutdown"}),
            "VLAN name",
        ),
        (
            "ios_access_port",
            json!({"interface": "Gi0/5", "vlan": 10, "description": "phones\n shutdown"}),
            "Description",
        ),
        (
            "ios_disable_port",
            json!({"interface": "Gi0/9", "reason": "spare\r shutdown"}),
            "Reason",
        ),
        (
            "ios_ntp",
            json!({"servers": ["192.0.2.1\nno ntp server 192.0.2.2"]}),
            "NTP servers",
        ),
        (
            "routeros_syslog",
            json!({"host": "10.0.0.5", "topics": "info\n/system reboot"}),
            "Topics",
        ),
        (
            "ios_dns",
            json!({"servers": ["1.1.1.1"], "domain": "lab.example\u{1b}[2J"}),
            "Domain name",
        ),
    ] {
        let err = lab
            .app
            .rpc(
                "templates.render",
                json!({"template_id": id, "device_ids": [lab.id("SW1")], "vars": vars}),
            )
            .await
            .expect_err("a second line in a one-line value is a second configuration line");
        assert_eq!(err.code, "invalid", "{id}");
        assert!(
            err.message.contains(label) && err.message.contains("one line"),
            "{id}: {}",
            err.message
        );
    }

    // The one value that is text over several lines says so, and keeps them.
    let rendered = lab
        .app
        .rpc(
            "templates.render",
            json!({"template_id": "ios_banner", "device_ids": [lab.id("SW1")],
                   "vars": {"text": "Authorised use only.\r\nDisconnect now."}}),
        )
        .await
        .expect("templates.render");
    assert_eq!(
        rendered[0]["lines"],
        json!([
            "banner login ^C",
            "Authorised use only.",
            "Disconnect now.",
            "^C"
        ])
    );
    // Lines, yes; a sequence that moves a cursor, or the banner's own end, no.
    for text in ["Welcome\u{1b}[2J", "Hello ^C\nno ip routing"] {
        let said = turned_away(&lab, "ios_banner", &json!({"text": text}))
            .await
            .unwrap_or_else(|what| panic!("{what}"));
        assert!(said.contains("Banner text"), "{said}");
    }
}

#[tokio::test]
async fn a_value_with_more_in_it_than_its_command_takes_is_turned_away() {
    // One line can still be two things: an interface and a `shutdown`, an
    // address and a `disabled=yes`, a topic and a second RouterOS command.
    let lab = lab(&["SW1"]).await;
    let lan = |extra: Value| {
        let mut vars = json!({
            "interface": "vlan30", "address": "10.30.0.1/24", "network": "10.30.0.0/24",
            "gateway": "10.30.0.1", "pool_from": "10.30.0.100", "pool_to": "10.30.0.200",
            "dns": ["1.1.1.1"]
        });
        vars.as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("object").clone());
        vars
    };
    let cases: Vec<(&str, Value)> = vec![
        // ----------------------------------- an interface and something more
        (
            "ios_ntp",
            json!({"servers": ["192.0.2.1"], "source": "Gi0/1 shutdown"}),
        ),
        (
            "ios_access_port",
            json!({"interface": "Gi0/5 shutdown", "vlan": 10}),
        ),
        (
            "ios_access_port",
            json!({"interface": "range Gi0/1 - 24", "vlan": 10}),
        ),
        (
            "ios_trunk_port",
            json!({"interface": "Gi0/1 shutdown", "vlans": [30]}),
        ),
        ("ios_disable_port", json!({"interface": "Gi0/9;shutdown"})),
        (
            "routeros_vlan_bridge",
            json!({"vlan": 30, "interface": "bridge disabled=yes"}),
        ),
        (
            "routeros_lan_with_dhcp",
            lan(json!({"interface": "ether2 disabled=yes"})),
        ),
        // ---------------------------- a name, a word, a list item that is more
        ("ios_vlan", json!({"vlan": 30, "vlan_name": "guests extra"})),
        (
            "ios_mgmt_acl_vty",
            json!({"acl_name": "MGMT VTY", "prefixes": ["10.0.0.0/24"]}),
        ),
        ("ios_mgmt_acl_vty", json!({"prefixes": ["any"]})),
        ("ios_mgmt_acl_vty", json!({"prefixes": ["10.0.0.0/24 log"]})),
        (
            "ios_dns",
            json!({"servers": ["1.1.1.1"], "domain": "lab.example shutdown"}),
        ),
        ("ios_dns", json!({"servers": ["1.1.1.1 vrf X"]})),
        ("ios_ntp", json!({"servers": ["192.0.2.1 prefer"]})),
        (
            "ios_syslog",
            json!({"hosts": ["10.0.0.5 transport tcp port 1470"]}),
        ),
        (
            "ios_trunk_port",
            json!({"interface": "Gi0/1", "vlans": ["30 remove"]}),
        ),
        (
            "routeros_syslog",
            json!({"host": "10.0.0.5", "topics": "info;/system reset-configuration"}),
        ),
        (
            "routeros_syslog",
            json!({"host": "10.0.0.5", "topics": "info disabled=yes"}),
        ),
        (
            "routeros_syslog",
            json!({"host": "10.0.0.5", "action_name": "remote disabled=yes"}),
        ),
        (
            "routeros_syslog",
            json!({"host": "10.0.0.5", "action_name": "[/system reboot]"}),
        ),
        (
            "routeros_vlan_bridge",
            json!({"vlan": 30, "vlan_name": "v30 disabled=yes"}),
        ),
        (
            "routeros_lan_with_dhcp",
            lan(json!({"lease_time": "1d disabled=yes"})),
        ),
        (
            "routeros_lan_with_dhcp",
            lan(json!({"dns": ["1.1.1.1;/system reboot"]})),
        ),
        (
            "routeros_ntp_client",
            json!({"servers": ["192.0.2.1;/system reboot"]}),
        ),
        ("routeros_ntp_client", json!({"servers": ["$server"]})),
        (
            "routeros_mgmt_services",
            json!({"prefixes": ["10.0.0.0/24 disabled=no"]}),
        ),
    ];
    let mut wrong = Vec::new();
    for (id, vars) in &cases {
        match turned_away(&lab, id, vars).await {
            Ok(said) if said.contains(char::is_control) => {
                wrong.push(format!("{id} {vars}: a control character in {said:?}"))
            }
            Ok(_) => {}
            Err(what) => wrong.push(what),
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

// ----------------------------------------------------------- device types

#[tokio::test]
async fn a_template_for_one_device_type_waits_until_a_devices_type_is_known_and_then_follows_it() {
    let lab = lab(&["SW1"]).await;
    // Left to detection: nothing says what it is until it has been reached.
    let gateway = lab
        .app
        .rpc(
            "devices.create",
            json!({"name": "GW1", "address": "192.0.2.2"}),
        )
        .await
        .expect("devices.create")["id"]
        .as_str()
        .expect("id")
        .to_string();
    let apply = |template: &str, vars: Value| {
        let app = lab.app.clone();
        let gateway = gateway.clone();
        let template = template.to_string();
        async move {
            app.rpc(
                "templates.apply",
                json!({"template_id": template, "device_ids": [gateway], "vars": vars}),
            )
            .await
        }
    };
    let tree = json!({"interface": "ether1", "rate_kbps": 20000});
    let shaper = json!({"interface": "Gi0/1", "rate_kbps": 50000});

    for (template, vars) in [
        ("routeros_qos_queue_tree", &tree),
        ("ios_qos_shaped_uplink", &shaper),
    ] {
        let err = apply(template, vars.clone())
            .await
            .expect_err("its type is not known");
        assert_eq!(err.code, "invalid", "{template}");
        assert!(
            err.message.contains("GW1") && err.message.contains("not known yet"),
            "{template}: {}",
            err.message
        );
    }
    assert_eq!(created(&lab), 0);

    // Detected as RouterOS: the RouterOS template is for it, the IOS one is not.
    inventory::update_state(&lab.app, &gateway, |state| {
        state.detected_profile = Some("mikrotik_routeros".into())
    })
    .expect("detected");
    let err = apply("ios_qos_shaped_uplink", shaper)
        .await
        .expect_err("written for IOS");
    assert!(
        err.message.contains("cisco_ios") && err.message.contains("GW1 (mikrotik_routeros)"),
        "{}",
        err.message
    );
    let applied = apply("routeros_qos_queue_tree", tree)
        .await
        .expect("templates.apply");
    assert_eq!(applied["change_id"], json!("chg-1"));
    assert_eq!(created(&lab), 1);
}

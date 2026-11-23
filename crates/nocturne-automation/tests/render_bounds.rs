//! What one rendering may write and do, set against what the built-in
//! templates need on a 48-port switch: the bounds have to stop a runaway
//! template without coming anywhere near a normal one.

mod common;

use common::lab;
use nocturne_app::{collect, inventory};
use nocturne_automation::{builtins, render, templates};
use nocturne_drivers::model::Kind;
use serde_json::{Value, json};

/// Values a person would give each built-in.
fn typical(id: &str) -> Value {
    match id {
        "ios_ntp" => json!({"servers": ["192.0.2.1", "192.0.2.2"], "source": "Loopback0"}),
        "ios_syslog" => json!({"hosts": ["10.0.0.5", "10.0.0.6"]}),
        "ios_banner" => json!({"text": "Authorised use only.\nDisconnect now if you are not."}),
        "ios_vlan" => json!({"vlan": 30, "vlan_name": "guests"}),
        "ios_access_port" => json!({"interface": "Gi1/0/5", "vlan": 10,
                                    "description": "desk 5", "voice_vlan": 110}),
        "ios_trunk_port" => json!({"interface": "Gi1/0/48", "vlans": [30, 31, 40],
                                   "native_vlan": 999, "description": "to CORE1"}),
        "ios_port_description_from_neighbor" | "ios_health_check" => json!({}),
        "ios_dns" => json!({"servers": ["1.1.1.1", "9.9.9.9"], "domain": "lab.example"}),
        "ios_disable_port" => json!({"interface": "Gi1/0/9"}),
        "ios_mgmt_acl_vty" => json!({"prefixes": ["10.0.0.0/24", "192.168.1.0/28"]}),
        "routeros_ntp_client" => json!({"servers": ["192.0.2.1", "192.0.2.2"]}),
        "routeros_syslog" => json!({"host": "10.0.0.5"}),
        "routeros_vlan_bridge" => json!({"vlan": 30}),
        "routeros_lan_with_dhcp" => json!({
            "interface": "vlan30", "address": "10.30.0.1/24", "network": "10.30.0.0/24",
            "gateway": "10.30.0.1", "pool_from": "10.30.0.100", "pool_to": "10.30.0.200",
            "dns": ["1.1.1.1", "9.9.9.9"]
        }),
        "routeros_mgmt_services" => json!({"prefixes": ["10.0.0.0/24", "10.1.0.0/24"]}),
        "ios_qos_wan_edge" => json!({"interface": "Gi0/1", "shape_kbps": 50000,
                                     "critical_dscp": [8, 7, 6, 5, 4, 3, "default", "CS3"]}),
        "ios_qos_shaped_uplink" => json!({"interface": "Gi0/1", "rate_kbps": 50000}),
        "ios_qos_access_port_dscp" => {
            json!({"interface": "Gi1/0/5", "mode": "set", "dscp": "af21"})
        }
        "routeros_qos_simple_queue" => json!({
            "queue_name": "sub-1042", "target": "192.168.88.251/32", "upload_kbps": 5000,
            "download_kbps": 15000, "guaranteed_upload_kbps": 1000,
            "guaranteed_download_kbps": 5000
        }),
        "routeros_qos_queue_tree" => json!({"interface": "ether1", "rate_kbps": 20000}),
        other => panic!("no typical values written down for `{other}`"),
    }
}

#[tokio::test]
async fn the_built_in_templates_on_a_48_port_switch_stay_far_inside_what_a_rendering_may_do() {
    let lab = lab(&["SW1"]).await;
    let ports: Vec<Value> = (1..=48)
        .map(|port| {
            json!({"name": format!("GigabitEthernet1/0/{port}"), "description": format!("desk {port}"),
                   "admin_up": true, "oper_up": port % 3 != 0, "speed_mbps": 1000})
        })
        .collect();
    let neighbors: Vec<Value> = (1..=48)
        .map(|port| {
            json!({"local_interface": format!("Gi1/0/{port}"), "device": format!("AP-{port:02}"),
                   "remote_interface": "GigabitEthernet0", "platform": "", "address": "",
                   "protocol": "cdp"})
        })
        .collect();
    collect::store(&lab.app, &lab.id("SW1"), Kind::Interfaces, ports, "test").expect("stored");
    collect::store(&lab.app, &lab.id("SW1"), Kind::Neighbors, neighbors, "test").expect("stored");
    let device = inventory::get(&lab.app, &lab.id("SW1")).expect("the device");

    // And one written the way people write their own: every port, a line or three each.
    let mut all = builtins::all();
    all.push(nocturne_automation::model::Template {
        id: "every_port".into(),
        body: "{% for port in interfaces %}interface {{ port.name }}\n description {{ port.description }}\n{% if not port.oper_up %} shutdown\n{% endif %}{% endfor %}".into(),
        ..Default::default()
    });
    for template in all {
        let vars = if template.id == "every_port" {
            json!({})
        } else {
            typical(&template.id)
        };
        let bound = templates::bind(&template.variables, &vars)
            .unwrap_or_else(|err| panic!("{}: {err}", template.id));
        let context = render::context(&lab.app, &device, &bound).expect("a context");
        let (lines, usage) =
            render::render_with_usage(&template.body, &context, &render::Budget::new())
                .unwrap_or_else(|err| panic!("{}: {err}", template.id));
        assert!(!lines.is_empty(), "{} rendered nothing", template.id);
        assert!(
            usage.bytes * 100 <= render::DEVICE_BYTES,
            "{} wrote {} bytes: not a hundredth of what a device may have",
            template.id,
            usage.bytes
        );
        assert!(
            usage.fuel * 20 <= render::DEVICE_FUEL,
            "{} took {} steps: not a twentieth of what a device may take",
            template.id,
            usage.fuel
        );
    }
}

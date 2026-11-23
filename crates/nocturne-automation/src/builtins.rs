//! Templates that ship with Nocturne.
//!
//! The bar for shipping one is "a real device accepts this": a template that
//! produces a line a switch rejects is worse than no template at all, because
//! it gets as far as a change before anybody finds out. Where a platform's
//! syntax differs between major versions the description says which one the
//! body is written for.
//!
//! **No built-in template takes a password, a community string or a key.** A
//! rendered line ends up in a change document, in job output and on screen;
//! secrets live in the vault and go nowhere near any of that. That is why
//! there is no local-user and no SNMPv3-user template here, which the
//! specification asks for — see `NOTES.md`.

use crate::model::{Template, TemplateKind, Variable, VariableKind};

mod qos;

/// The start of every IOS body that takes an interface: the interface types
/// IOS writes in its configuration, and `ios_interface(value, label,
/// example)`, which gives back the name the router will show or refuses it.
///
/// IOS takes any unambiguous start of a type (`Gigabit0/2`, `TenGig1/1/1`)
/// and shows the full name afterwards. `ifname` expands the usual short forms
/// (`Gi`, `Te`, `Po`, …) and leaves anything else as typed, and a line
/// rendered as typed is predicted by the change pipeline as a section the
/// device never has: the change is rolled back, and the rollback resets the
/// real port with `default interface`. So after `ifname` the name has to
/// start with a type written out in full; its case is put right, nothing else
/// is guessed. A type that is not on the list is refused too: a copy of the
/// template can add it. It is part of the body, as the checks are, so that a
/// copy keeps it.
pub(crate) const IOS_INTERFACE: &str = r#"
{% set ios_types = ['GigabitEthernet', 'FastEthernet', 'TenGigabitEthernet', 'TwoGigabitEthernet', 'FiveGigabitEthernet', 'TwentyFiveGigE', 'FortyGigabitEthernet', 'HundredGigE', 'AppGigabitEthernet', 'Ethernet', 'Port-channel', 'Vlan', 'Loopback', 'Tunnel', 'Serial', 'Dialer', 'Multilink', 'Virtual-Template', 'Cellular', 'ATM', 'Dot11Radio', 'BDI', 'BVI'] %}
{% macro ios_interface(value, label, example) %}{% set typed = value | ifname %}{% set found = namespace(name='') %}{% if typed | length <= 64 %}{% for type in ios_types %}{% if typed is matching('(?i)^' ~ type ~ '[0-9][0-9/.:]*$') %}{% set found.name = type ~ typed[(type | length):] %}{% endif %}{% endfor %}{% endif %}{% if not found.name %}{{ fail(label ~ " must be one interface, by the full name the router gives it in its configuration, such as " ~ example ~ ": `" ~ value ~ "` is not one") }}{% endif %}{{ found.name }}{% endmacro %}
"#;

/// Checks for values that go into a command as one word, so that a value
/// cannot carry a second parameter (`ether2 disabled=yes`), a second line, or
/// on RouterOS a second command (`;`) or one in brackets: `word(value, label,
/// pattern, what)` gives the value back or refuses it, `words(values, …)`
/// refuses a list with an item that is not one.
pub(crate) const WORDS: &str = r#"
{% macro word(value, label, pattern, what) %}{% if value | string is not matching(pattern) %}{{ fail(label ~ " must be " ~ what ~ ": `" ~ value ~ "` is not") }}{% endif %}{{ value }}{% endmacro %}
{% macro words(values, label, pattern, what) %}{% for value in values %}{% if value | string is not matching(pattern) %}{{ fail(label ~ ": `" ~ value ~ "` is not " ~ what) }}{% endif %}{% endfor %}{% endmacro %}
"#;

/// The preludes a body needs, then the body.
fn with(preludes: &[&str], body: &str) -> String {
    let mut text: String = preludes.concat();
    text.push_str(body);
    text
}

fn var(name: &str, label: &str, kind: VariableKind, required: bool) -> Variable {
    Variable {
        name: name.to_string(),
        label: label.to_string(),
        kind,
        required,
        ..Variable::default()
    }
}

fn with_default(mut variable: Variable, default: serde_json::Value) -> Variable {
    variable.default = Some(default);
    variable
}

fn with_help(mut variable: Variable, help: &str) -> Variable {
    variable.help = help.to_string();
    variable
}

fn template(
    id: &str,
    name: &str,
    description: &str,
    kind: TemplateKind,
    profiles: &[&str],
    variables: Vec<Variable>,
    body: &str,
) -> Template {
    Template {
        id: id.to_string(),
        name: name.to_string(),
        description: description.to_string(),
        kind,
        body: body.trim_start_matches('\n').to_string(),
        builtin: true,
        profiles: profiles.iter().map(|p| p.to_string()).collect(),
        variables,
        tags: Vec::new(),
        updated_at: 0,
    }
}

/// Every built-in, in a stable order.
pub fn all() -> Vec<Template> {
    let mut all = general();
    all.extend(qos::all());
    all
}

fn general() -> Vec<Template> {
    use VariableKind::*;
    vec![
        // ------------------------------------------------------------- IOS
        template(
            "ios_ntp",
            "NTP servers (IOS)",
            "Point the device at one or more NTP servers.",
            TemplateKind::Config,
            &["cisco_ios"],
            vec![
                with_help(
                    var("servers", "NTP servers", List, true),
                    "One or more IP addresses.",
                ),
                with_default(
                    var("source", "Source interface", Interface, false),
                    "".into(),
                ),
            ],
            &with(
                &[IOS_INTERFACE, WORDS],
                r#"
{{ words(servers, "NTP servers", '^[A-Za-z0-9.:-]{1,253}$', "one address or host name") }}
{% for server in servers %}
ntp server {{ server }}
{% endfor %}
{% if source %}
ntp source {{ ios_interface(source, "Source interface", "Loopback0") }}
{% endif %}
"#,
            ),
        ),
        template(
            "ios_syslog",
            "Syslog hosts (IOS)",
            "Send logs to one or more collectors.",
            TemplateKind::Config,
            &["cisco_ios"],
            vec![
                var("hosts", "Syslog hosts", List, true),
                Variable {
                    choices: [
                        "emergencies",
                        "alerts",
                        "critical",
                        "errors",
                        "warnings",
                        "notifications",
                        "informational",
                        "debugging",
                    ]
                    .iter()
                    .map(|choice| choice.to_string())
                    .collect(),
                    ..with_default(
                        var("severity", "Trap level", Choice, false),
                        "informational".into(),
                    )
                },
            ],
            &with(
                &[WORDS],
                r#"
{{ words(hosts, "Syslog hosts", '^[A-Za-z0-9.:-]{1,253}$', "one address or host name") }}
{% for host in hosts %}
logging host {{ host }}
{% endfor %}
logging trap {{ severity }}
"#,
            ),
        ),
        template(
            "ios_banner",
            "Login banner (IOS)",
            "A login banner delimited with ^C.",
            TemplateKind::Config,
            &["cisco_ios"],
            vec![Variable {
                multiline: true,
                ..with_help(
                    var("text", "Banner text", String, true),
                    "As many lines as it needs. It may not hold ^, which is what ends it.",
                )
            }],
            r#"
{% if '^' in text %}{{ fail("Banner text must not hold `^`: that is what ends the banner, and the lines after it would be read as configuration") }}{% endif %}
banner login ^C
{{ text }}
^C
"#,
        ),
        template(
            "ios_vlan",
            "Add a VLAN (IOS)",
            "Create a VLAN and name it.",
            TemplateKind::Config,
            &["cisco_ios"],
            vec![
                var("vlan", "VLAN id", Vlan, true),
                var("vlan_name", "VLAN name", String, true),
            ],
            &with(
                &[WORDS],
                r#"
vlan {{ vlan }}
 name {{ word(vlan_name, "VLAN name", '^[A-Za-z0-9_.:/+@#-]{1,32}$', "one word of 32 characters at most") }}
"#,
            ),
        ),
        template(
            "ios_access_port",
            "Access port (IOS)",
            "An access port with a description, PortFast and an optional voice VLAN.",
            TemplateKind::Config,
            &["cisco_ios"],
            vec![
                var("interface", "Interface", Interface, true),
                var("vlan", "Access VLAN", Vlan, true),
                with_default(var("description", "Description", String, false), "".into()),
                with_default(var("voice_vlan", "Voice VLAN", Int, false), 0.into()),
            ],
            &with(
                &[IOS_INTERFACE],
                r#"
interface {{ ios_interface(interface, "Interface", "GigabitEthernet1/0/5") }}
{% if description %}
 description {{ description }}
{% endif %}
 switchport mode access
 switchport access vlan {{ vlan }}
{% if voice_vlan %}
 switchport voice vlan {{ voice_vlan }}
{% endif %}
 spanning-tree portfast
"#,
            ),
        ),
        template(
            "ios_trunk_port",
            "Trunk port (IOS)",
            "Add VLANs to a trunk. Always `switchport trunk allowed vlan add`: the \
             form without `add` replaces the whole list and takes every other VLAN off \
             the trunk.",
            TemplateKind::Config,
            &["cisco_ios"],
            vec![
                var("interface", "Interface", Interface, true),
                with_help(
                    var("vlans", "VLANs to allow", List, true),
                    "Added to whatever the trunk already allows.",
                ),
                with_default(var("native_vlan", "Native VLAN", Int, false), 0.into()),
                with_default(var("description", "Description", String, false), "".into()),
            ],
            &with(
                &[IOS_INTERFACE, WORDS],
                r#"
{{ words(vlans, "VLANs to allow", '^[0-9]{1,4}(-[0-9]{1,4})?$', "a VLAN id or a range of them (30, 40-49)") }}
interface {{ ios_interface(interface, "Interface", "GigabitEthernet1/0/48") }}
{% if description %}
 description {{ description }}
{% endif %}
 switchport mode trunk
{% if native_vlan %}
 switchport trunk native vlan {{ native_vlan }}
{% endif %}
 switchport trunk allowed vlan add {{ vlans | join(',') }}
"#,
            ),
        ),
        template(
            "ios_port_description_from_neighbor",
            "Describe ports from LLDP/CDP (IOS)",
            "Give every port with a discovered neighbour a description naming it. \
             Needs collected neighbours.",
            TemplateKind::Config,
            &["cisco_ios"],
            vec![with_default(
                var("prefix", "Description prefix", String, false),
                "to".into(),
            )],
            r#"
{% for neighbor in neighbors %}
interface {{ neighbor.local_interface | ifname }}
 description {{ prefix }} {{ neighbor.device }} {{ neighbor.remote_interface }}
{% endfor %}
"#,
        ),
        template(
            "ios_dns",
            "DNS servers (IOS)",
            "Resolver addresses and the domain name.",
            TemplateKind::Config,
            &["cisco_ios"],
            vec![
                var("servers", "Name servers", List, true),
                with_default(var("domain", "Domain name", String, false), "".into()),
            ],
            &with(
                &[WORDS],
                r#"
{{ words(servers, "Name servers", '^[0-9A-Fa-f.:]{2,45}$', "one IP address") }}
{% for server in servers %}
ip name-server {{ server }}
{% endfor %}
{% if domain %}
ip domain-name {{ word(domain, "Domain name", '^[A-Za-z0-9.-]{1,253}$', "one domain name") }}
{% endif %}
"#,
            ),
        ),
        template(
            "ios_disable_port",
            "Disable an unused port (IOS)",
            "Shut a port and say why in its description.",
            TemplateKind::Config,
            &["cisco_ios"],
            vec![
                var("interface", "Interface", Interface, true),
                with_default(var("reason", "Reason", String, false), "unused".into()),
            ],
            &with(
                &[IOS_INTERFACE],
                r#"
interface {{ ios_interface(interface, "Interface", "GigabitEthernet1/0/9") }}
 description DISABLED - {{ reason }}
 shutdown
"#,
            ),
        ),
        template(
            "ios_mgmt_acl_vty",
            "Management ACL on VTY (IOS)",
            "Allow SSH to the device only from the management prefixes. Review the \
             change carefully: an ACL that leaves out your own address locks you out.",
            TemplateKind::Config,
            &["cisco_ios"],
            vec![
                with_default(
                    var("acl_name", "ACL name", String, false),
                    "MGMT-VTY".into(),
                ),
                with_help(
                    var("prefixes", "Allowed prefixes", List, true),
                    "In CIDR form, e.g. 10.0.0.0/24.",
                ),
            ],
            &with(
                &[WORDS],
                r#"
{% set acl = word(acl_name, "ACL name", '^[A-Za-z][A-Za-z0-9_-]{0,63}$', "one name that starts with a letter, 64 characters at most") %}
{{ words(prefixes, "Allowed prefixes", '^[0-9]{1,3}([.][0-9]{1,3}){3}/[0-9]{1,2}$', "an IPv4 prefix such as 10.0.0.0/24") }}
ip access-list standard {{ acl }}
{% for prefix in prefixes %}
 permit {{ prefix | regex_replace('/.*', '') }} {{ prefix | wildcard }}
{% endfor %}
line vty 0 15
 access-class {{ acl }} in
 transport input ssh
"#,
            ),
        ),
        template(
            "ios_health_check",
            "Health check (IOS)",
            "Read-only commands worth running before and after a change.",
            TemplateKind::Commands,
            &["cisco_ios"],
            vec![],
            r#"
show version
show ip interface brief
show cdp neighbors
show logging
"#,
        ),
        // -------------------------------------------------------- RouterOS
        template(
            "routeros_ntp_client",
            "NTP client (RouterOS 7)",
            "RouterOS 7 syntax (`/system ntp client`). RouterOS 6 uses \
             `primary-ntp`/`secondary-ntp` instead.",
            TemplateKind::Config,
            &["mikrotik_routeros"],
            vec![var("servers", "NTP servers", List, true)],
            &with(
                &[WORDS],
                r#"
{{ words(servers, "NTP servers", '^[A-Za-z0-9.:-]{1,253}$', "one address or host name") }}
/system ntp client set enabled=yes servers={{ servers | join(',') }}
"#,
            ),
        ),
        template(
            "routeros_syslog",
            "Remote syslog (RouterOS)",
            "A remote logging action and the topics sent to it.",
            TemplateKind::Config,
            &["mikrotik_routeros"],
            vec![
                var("host", "Syslog host", Ip, true),
                with_default(var("port", "Port", Int, false), 514.into()),
                with_default(var("topics", "Topics", String, false), "info".into()),
                with_default(
                    var("action_name", "Action name", String, false),
                    "nocturne-remote".into(),
                ),
            ],
            &with(
                &[WORDS],
                r#"
{% set action = word(action_name, "Action name", '^[A-Za-z0-9_.-]{1,64}$', "one name of letters, digits and . _ -, 64 characters at most") %}
/system logging action add name={{ action }} target=remote remote={{ host }} remote-port={{ port }}
/system logging add action={{ action }} topics={{ word(topics, "Topics", '^!?[a-z0-9-]+(,!?[a-z0-9-]+)*$', "topic names separated by commas, such as info or critical,!debug") }}
"#,
            ),
        ),
        template(
            "routeros_vlan_bridge",
            "VLAN interface on a bridge (RouterOS)",
            "A tagged VLAN interface on an existing bridge.",
            TemplateKind::Config,
            &["mikrotik_routeros"],
            vec![
                var("vlan", "VLAN id", Vlan, true),
                with_default(
                    var("interface", "Parent interface", Interface, false),
                    "bridge".into(),
                ),
                with_default(var("vlan_name", "Interface name", String, false), "".into()),
            ],
            &with(
                &[WORDS],
                r#"
{% set name = word(vlan_name or ('vlan' ~ vlan), "Interface name", '^[A-Za-z0-9_.@:/+-]{1,64}$', "one RouterOS name, 64 characters at most") %}
/interface vlan add name={{ name }} vlan-id={{ vlan }} interface={{ word(interface, "Parent interface", '^[A-Za-z0-9_.@:/+-]{1,64}$', "one RouterOS interface name, such as bridge") }}
"#,
            ),
        ),
        template(
            "routeros_lan_with_dhcp",
            "Address and DHCP server for a new LAN (RouterOS)",
            "An address on an interface, a pool, the network record and the DHCP \
             server itself. Every value is explicit: nothing is guessed from the prefix.",
            TemplateKind::Config,
            &["mikrotik_routeros"],
            vec![
                var("interface", "Interface", Interface, true),
                with_help(
                    var("address", "Router address", Cidr, true),
                    "The router's own address, e.g. 10.20.0.1/24.",
                ),
                with_help(var("network", "Network", Cidr, true), "e.g. 10.20.0.0/24."),
                var("gateway", "Gateway", Ip, true),
                var("pool_from", "Pool from", Ip, true),
                var("pool_to", "Pool to", Ip, true),
                var("dns", "DNS servers", List, true),
                with_default(var("lease_time", "Lease time", String, false), "1d".into()),
            ],
            &with(
                &[WORDS],
                r#"
{% set port = word(interface, "Interface", '^[A-Za-z0-9_.@:/+-]{1,64}$', "one RouterOS interface name, such as vlan30") %}
{% set lease = word(lease_time, "Lease time", '^[0-9wdhms:]{1,16}$', "a RouterOS time such as 1d or 00:30:00") %}
{{ words(dns, "DNS servers", '^[0-9A-Fa-f.:]{2,45}$', "one IP address") }}
/ip address add address={{ address }} interface={{ port }}
/ip pool add name=pool-{{ port }} ranges={{ pool_from }}-{{ pool_to }}
/ip dhcp-server network add address={{ network }} gateway={{ gateway }} dns-server={{ dns | join(',') }}
/ip dhcp-server add name=dhcp-{{ port }} interface={{ port }} address-pool=pool-{{ port }} lease-time={{ lease }} disabled=no
"#,
            ),
        ),
        template(
            "routeros_mgmt_services",
            "Restrict services to a management subnet (RouterOS)",
            "Limit the device's own services to the management prefixes and turn off \
             the cleartext ones. Review the change carefully: the wrong prefix locks \
             you out.",
            TemplateKind::Config,
            &["mikrotik_routeros"],
            vec![with_help(
                var("prefixes", "Allowed prefixes", List, true),
                "In CIDR form; the list replaces whatever is set.",
            )],
            &with(
                &[WORDS],
                r#"
{{ words(prefixes, "Allowed prefixes", '^[0-9A-Fa-f.:]+/[0-9]{1,3}$', "a prefix in CIDR form, such as 10.0.0.0/24") }}
/ip service set ssh address={{ prefixes | join(',') }}
/ip service set winbox address={{ prefixes | join(',') }}
/ip service set api address={{ prefixes | join(',') }}
/ip service set telnet disabled=yes
/ip service set ftp disabled=yes
/ip service set www disabled=yes
"#,
            ),
        ),
    ]
}

pub fn get(id: &str) -> Option<Template> {
    all().into_iter().find(|template| template.id == id)
}

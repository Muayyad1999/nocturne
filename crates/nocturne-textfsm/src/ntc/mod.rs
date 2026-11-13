//! The bundled ntc-templates library.
//!
//! The templates and the `index` file of
//! [networktocode/ntc-templates](https://github.com/networktocode/ntc-templates)
//! are embedded in the crate. Commands are resolved through the index exactly
//! like `ntc_templates.parse.parse_output` does (which delegates to
//! `textfsm.clitable`), and templates are compiled on first use.

mod bundle;
mod index;
mod merge;

use std::sync::{Arc, OnceLock};

pub use merge::MergeError;

use bundle::Bundle;
use index::Index;

use crate::{Error, Record, Template, TemplateError};

fn index() -> &'static Index<'static> {
    static INDEX: OnceLock<Index<'static>> = OnceLock::new();
    INDEX.get_or_init(|| Index::parse(Bundle::get().index()))
}

/// Platforms present in the index, e.g. "cisco_ios", "arista_eos", "mikrotik_routeros".
///
/// The list is sorted. The index describes platforms with regular expressions;
/// alternatives are listed individually (`hp_procurve` and `aruba_procurve`),
/// and a platform given to [`find`] only has to start with one of these names.
pub fn platforms() -> &'static [String] {
    static PLATFORMS: OnceLock<Vec<String>> = OnceLock::new();
    PLATFORMS.get_or_init(|| index().platforms())
}

/// All bundled template names (file stem, e.g. "cisco_ios_show_version"), sorted.
pub fn template_names() -> impl Iterator<Item = &'static str> {
    Bundle::get().names()
}

/// The source text of a bundled template.
pub fn source(name: &str) -> Option<&'static str> {
    Bundle::get().source(name)
}

/// A bundled template by name. Compiled lazily, cached for the life of the process.
pub fn template(name: &str) -> Result<Option<Arc<Template>>, TemplateError> {
    Bundle::get().template(name)
}

/// Resolve via the ntc `index` file: platform + the command as typed.
///
/// Abbreviations resolve as upstream: `sh ip int br` finds
/// `cisco_ios_show_ip_interface_brief`. Like upstream, the index entries are
/// matched against the *start* of the command, so trailing arguments or
/// output modifiers do not prevent a match, and the first matching entry wins.
///
/// A few commands are covered by several templates whose columns are
/// combined; this returns the first one (see [`find_all`]).
pub fn find(platform: &str, command: &str) -> Option<&'static str> {
    find_all(platform, command).and_then(|templates| templates.first().copied())
}

/// Like [`find`], but returns every template the index lists for the command.
///
/// [`parse`] runs all of them and merges their columns.
pub fn find_all(platform: &str, command: &str) -> Option<&'static [&'static str]> {
    // Python's `$` also matches in front of a final newline.
    let command = command.strip_suffix('\n').unwrap_or(command);
    index().find(platform, command)
}

/// Convenience: find + run. `Ok(None)` when no template covers the command.
///
/// When the index lists several templates for the command, the columns the
/// later ones add are merged into the rows of the first, matched on the
/// values declared as `Key` — the same table `parse_output` returns upstream.
pub fn parse(platform: &str, command: &str, output: &str) -> Result<Option<Vec<Record>>, Error> {
    let Some(names) = find_all(platform, command) else {
        return Ok(None);
    };
    let mut templates = Vec::with_capacity(names.len());
    for name in names {
        match template(name)? {
            Some(template) => templates.push(template),
            None => return Ok(None),
        }
    }
    let Some((first, rest)) = templates.split_first() else {
        return Ok(None);
    };
    let mut rows = first.run(output)?;
    let mut header = first.header().to_vec();
    let mut keys: Vec<String> = first.keys().map(str::to_owned).collect();
    for template in rest {
        let other = template.run(output)?;
        if keys.is_empty() {
            keys = template.keys().map(str::to_owned).collect();
        }
        merge::extend(&mut rows, &mut header, &other, template.header(), &keys)?;
    }
    Ok(Some(rows))
}

/// Upstream commit the bundle was taken from.
pub fn upstream_revision() -> &'static str {
    bundle::UPSTREAM_REVISION
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::Value;

    /// `(platform, command, templates)` as resolved by `textfsm.clitable`
    /// with the bundled index.
    const RESOLUTIONS: &[(&str, &str, &[&str])] = &[
        // Abbreviations expand the way the index spells them out.
        ("cisco_ios", "sh ip int br", &["cisco_ios_show_ip_interface_brief"]),
        ("cisco_ios", "show ip interface brief", &["cisco_ios_show_ip_interface_brief"]),
        ("cisco_ios", "sh ver", &["cisco_ios_show_version"]),
        ("cisco_ios", "s ver", &[]),
        ("cisco_ios", "sh int", &["cisco_ios_show_interfaces"]),
        ("arista_eos", "sh ip bgp sum", &["arista_eos_show_ip_bgp_summary"]),
        ("huawei_vrp", "dis vlan brief", &["huawei_vrp_display_vlan_brief"]),
        // Only the start of the command has to match ...
        ("cisco_ios", "sho ver | include uptime", &["cisco_ios_show_version"]),
        ("cisco_ios", "show vers extra", &["cisco_ios_show_version"]),
        ("cisco_ios", "show int gi0/1", &["cisco_ios_show_interfaces"]),
        ("cisco_ios", "show ip route vrf X", &["cisco_ios_show_ip_route"]),
        ("cisco_ios", "show version\n", &["cisco_ios_show_version"]),
        // ... unless the index entry is anchored.
        ("cisco_ios", "show switch detail extra", &[]),
        ("alcatel_aos", "show interfaces", &["alcatel_aos_show_interfaces_ethernet"]),
        ("alcatel_aos", "show interfaces status", &["alcatel_aos_show_interfaces_status"]),
        // The first matching entry wins; longer commands are listed first.
        ("cisco_ios", "show interfaces status", &["cisco_ios_show_interfaces_status"]),
        ("cisco_ios", "show ip bgp", &["cisco_ios_show_ip_bgp"]),
        ("cisco_ios", "show ip bgp summary", &["cisco_ios_show_ip_bgp_summary"]),
        (
            "cisco_ios",
            "show ip bgp neighbors 10.0.0.1 advertised-routes",
            &["cisco_ios_show_ip_bgp_neighbors_advertised-routes"],
        ),
        ("cisco_ios", "show mac address-table", &["cisco_ios_show_mac-address-table"]),
        ("cisco_ios", "show mac-address-table", &["cisco_ios_show_mac-address-table"]),
        // Matching is case sensitive and starts at the first character.
        ("cisco_ios", "Show version", &[]),
        ("cisco_ios", " show version", &[]),
        ("cisco_ios", "", &[]),
        ("cisco_ios", "show running-config", &[]),
        // Platforms are regular expressions matched against a prefix too.
        ("cisco_iosxe", "show version", &["cisco_ios_show_version"]),
        ("cisco", "show version", &[]),
        ("unknown_platform", "show version", &[]),
        ("hp_procurve", "show interfaces brief", &["hp_procurve_show_interfaces_brief"]),
        ("aruba_procurve", "show interfaces brief", &["hp_procurve_show_interfaces_brief"]),
        ("vyatta_vyos", "show interfaces", &["vyatta_vyos_show_interfaces"]),
        ("vyos", "show interfaces", &["vyatta_vyos_show_interfaces"]),
        ("my_vyos_box", "show interfaces", &["vyatta_vyos_show_interfaces"]),
        ("cisco_asa", "show interface", &["cisco_asa_show_interface"]),
        ("cisco_ftd", "show interface", &[]),
        // One platform each.
        ("cisco_nxos", "show version", &["cisco_nxos_show_version"]),
        ("cisco_xr", "show version", &["cisco_xr_show_version"]),
        ("arista_eos", "show interfaces status", &["arista_eos_show_interfaces_status"]),
        ("juniper_junos", "show interfaces", &["juniper_junos_show_interfaces"]),
        ("huawei_vrp", "display interface", &["huawei_vrp_display_interface"]),
        ("hp_comware", "display interface", &["hp_comware_display_interface"]),
        ("aruba_aoscx", "show interface", &["aruba_aoscx_show_interface"]),
        ("fortinet", "get system status", &["fortinet_get_system_status"]),
        ("paloalto_panos", "show system info", &["paloalto_panos_show_system_info"]),
        ("linux", "ip address show", &["linux_ip_address_show"]),
        ("mikrotik_routeros", "interface print detail", &["mikrotik_routeros_interface_print_detail"]),
        ("mikrotik_routeros", "/interface print detail", &["mikrotik_routeros_interface_print_detail"]),
        ("mikrotik_routeros", "ip address print", &["mikrotik_routeros_ip_address_print"]),
        ("mikrotik_routeros", "ip route print detail", &["mikrotik_routeros_ip_route_print_detail"]),
        ("mikrotik_routeros", "ip neighbor print detail", &["mikrotik_routeros_ip_neighbor_print_detail"]),
        ("mikrotik_routeros", "ip arp print", &["mikrotik_routeros_ip_arp_print"]),
        // Entries that combine several templates.
        (
            "cisco_ios",
            "show switch",
            &["cisco_ios_show_switch_detail", "cisco_ios_show_switch_detail_stack_ports"],
        ),
        (
            "cisco_ios",
            "show switch detail",
            &["cisco_ios_show_switch_detail", "cisco_ios_show_switch_detail_stack_ports"],
        ),
        (
            "cisco_ios",
            "show module",
            &[
                "cisco_ios_show_module",
                "cisco_ios_show_module_status",
                "cisco_ios_show_module_submodule",
                "cisco_ios_show_module_online_diag",
            ],
        ),
        ("huawei_vrp", "display vlan", &["huawei_vrp_display_vlan", "huawei_vrp_display_vlan_brief"]),
        (
            "huawei_smartax",
            "display ont info 0 1 2 all",
            &["huawei_smartax_display_ont_info_0_1_2", "huawei_smartax_display_ont_info_description"],
        ),
    ];

    #[test]
    fn resolves_commands_like_upstream() {
        for &(platform, command, templates) in RESOLUTIONS {
            let context = format!("{platform}: {command:?}");
            assert_eq!(find_all(platform, command).unwrap_or_default(), templates, "{context}");
            assert_eq!(find(platform, command), templates.first().copied(), "{context}");
        }
    }

    #[test]
    fn parses_with_the_resolved_template() {
        let output = "Interface    IP-Address  OK? Method Status                Protocol\n\
                      Vlan1        10.0.0.1    YES NVRAM  up                    up\n\
                      Gi0/1        unassigned  YES unset  administratively down down\n";
        let rows = parse("cisco_ios", "sh ip int br", output).unwrap().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[1].keys().collect::<Vec<_>>(),
            ["INTERFACE", "IP_ADDRESS", "STATUS", "PROTO"]
        );
        assert_eq!(rows[0]["IP_ADDRESS"], Value::from("10.0.0.1"));
        assert_eq!(rows[1]["STATUS"].as_str(), "administratively down");

        assert_eq!(parse("cisco_ios", "show running-config", output).unwrap(), None);
        assert_eq!(parse("no_such_platform", "show version", output).unwrap(), None);
    }

    #[test]
    fn merges_the_columns_of_every_template_of_an_entry() {
        // The first template yields one row per switch, the second one adds
        // the stack ports of the switches it finds, matched on the Key SWITCH.
        let output = "\
Switch/Stack Mac Address : 6c71.abcd.a200  - Local Mac Address
Mac persistency wait time: Indefinite
                                             H/W   Current
Switch#   Role    Mac Address     Priority Version  State 
-------------------------------------------------------------------------------------
*1       Active   6c71.abcd.a200     1      V03     Ready
 2       Member   0000.0000.0000     0              Unprovisioned                       



         Stack Port Status             Neighbors     
Switch#  Port 1     Port 2           Port 1   Port 2 
--------------------------------------------------------
  1       DOWN       DOWN             None     None 
";
        let rows = parse("cisco_ios", "show switch detail", output).unwrap().unwrap();
        let cells = |row: &crate::Record| {
            row.iter().map(|(name, value)| format!("{name}={}", value.as_str())).collect::<Vec<_>>()
        };
        assert_eq!(
            cells(&rows[0]),
            [
                "SWITCH=1",
                "ROLE=Active",
                "MAC_ADDRESS=6c71.abcd.a200",
                "PRIORITY=1",
                "VERSION=V03",
                "STATE=Ready",
                "STATUS1=DOWN",
                "STATUS2=DOWN",
                "NEIGHBOR1=None",
                "NEIGHBOR2=None",
            ]
        );
        assert_eq!(
            cells(&rows[1]),
            [
                "SWITCH=2",
                "ROLE=Member",
                "MAC_ADDRESS=0000.0000.0000",
                "PRIORITY=0",
                "VERSION=",
                "STATE=Unprovisioned",
                "STATUS1=",
                "STATUS2=",
                "NEIGHBOR1=",
                "NEIGHBOR2=",
            ]
        );
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn exposes_the_bundle() {
        let names: Vec<&str> = template_names().collect();
        assert_eq!(names.len(), 979);
        assert!(names.is_sorted() && names.contains(&"mikrotik_routeros_ip_address_print"));
        assert!(source("arista_eos_show_version").unwrap().contains("Start"));
        assert_eq!(source("arista_eos_show_version.textfsm"), None);
        assert!(template("no_such_template").unwrap().is_none());

        let template = template("cisco_ios_show_clock").unwrap().unwrap();
        assert_eq!(template.header(), ["TIME", "TIMEZONE", "DAYWEEK", "MONTH", "DAY", "YEAR"]);

        assert_eq!(upstream_revision().len(), 40);
        assert!(upstream_revision().bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(platforms().iter().any(|platform| platform == "cisco_ios"));
    }

    /// Also exercises the lazy cache from several threads at once.
    #[test]
    fn every_bundled_template_compiles() {
        let names: Vec<&str> = template_names().collect();
        let threads = std::thread::available_parallelism().map_or(4, |count| count.get());
        std::thread::scope(|scope| {
            for chunk in names.chunks(names.len().div_ceil(threads)) {
                scope.spawn(move || {
                    for name in chunk {
                        if let Err(error) = template(name) {
                            panic!("{name}: {error}");
                        }
                    }
                });
            }
        });
    }
}

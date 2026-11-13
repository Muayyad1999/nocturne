//! Reading vendor text the same way every time.
//!
//! Collected values come out of text parsers, so "full duplex" arrives as
//! `Full-duplex`, `a-full`, `full` or nothing at all depending on the platform
//! and the command. Every check normalises through here, and anything this
//! module cannot place becomes [`Duplex::Unresolved`] / `None` rather than a
//! guess — a check that guesses is a check that cries wolf.

use std::collections::BTreeSet;

use nocturne_drivers::hier::{Node, Syntax, parse_vlan_ranges};
use nocturne_drivers::model::Interface;

use crate::world::{Config, DeviceData};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Duplex {
    Full,
    Half,
    /// Still negotiating, not reported, or spelled in a way this does not know.
    Unresolved,
}

impl Duplex {
    pub fn as_str(self) -> &'static str {
        match self {
            Duplex::Full => "full",
            Duplex::Half => "half",
            Duplex::Unresolved => "unresolved",
        }
    }
}

/// `Full-duplex`, `a-full`, `FULL`, `auto-full` → full; `auto`, `` → unresolved.
///
/// `yes`/`no` (how RouterOS prints `full-duplex`) is deliberately *not*
/// resolved: a profile that maps a boolean into this field without saying
/// which way round it is would give the duplex checks their worst false
/// positives. Profiles are expected to map duplex to `full`/`half`/`auto`.
pub fn duplex(text: &str) -> Duplex {
    let text = text.trim().to_ascii_lowercase();
    let text = text
        .strip_suffix("-duplex")
        .or_else(|| text.strip_suffix(" duplex"))
        .unwrap_or(&text);
    let text = text
        .strip_prefix("a-")
        .or_else(|| text.strip_prefix("auto-"))
        .or_else(|| text.strip_prefix("auto "))
        .unwrap_or(text);
    match text {
        "full" | "fdx" => Duplex::Full,
        "half" | "hdx" => Duplex::Half,
        _ => Duplex::Unresolved,
    }
}

/// A speed for a person: `1 Gb/s`, `100 Mb/s`, `10.5 Gb/s`.
pub fn speed(bps: u64) -> String {
    const G: u64 = 1_000_000_000;
    const M: u64 = 1_000_000;
    const K: u64 = 1_000;
    let (unit, scale) = if bps >= G {
        ("Gb/s", G)
    } else if bps >= M {
        ("Mb/s", M)
    } else if bps >= K {
        ("kb/s", K)
    } else {
        return format!("{bps} b/s");
    };
    if bps.is_multiple_of(scale) {
        format!("{} {unit}", bps / scale)
    } else {
        format!("{:.1} {unit}", bps as f64 / scale as f64)
    }
}

/// What a switch port is doing, from the collected mode first and the
/// configuration second. `None` when neither says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Access,
    Trunk,
    Routed,
}

pub fn mode(data: &DeviceData, interface: &Interface) -> Option<Mode> {
    match interface.mode.trim().to_ascii_lowercase().as_str() {
        "access" | "static access" => return Some(Mode::Access),
        "trunk" => return Some(Mode::Trunk),
        "routed" | "layer3" | "l3" => return Some(Mode::Routed),
        _ => {}
    }
    let section = ios_section(data, &interface.name)?;
    let line = |prefix: &str| section.children.iter().any(|child| child.text == prefix);
    if line("switchport mode trunk") {
        return Some(Mode::Trunk);
    }
    if line("switchport mode access") {
        return Some(Mode::Access);
    }
    if section
        .children
        .iter()
        .any(|child| child.text.starts_with("ip address "))
        && !section
            .children
            .iter()
            .any(|child| child.text.starts_with("switchport"))
    {
        return Some(Mode::Routed);
    }
    None
}

/// The interface's configuration section, but only for grammars whose
/// `switchport` syntax this crate reads.
pub fn ios_section<'a>(data: &'a DeviceData, interface: &str) -> Option<&'a Node> {
    let config = data.config.as_ref()?;
    (config.syntax == Syntax::Ios)
        .then(|| config.interface(interface))
        .flatten()
}

/// Which VLANs a trunk carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Allowed {
    /// No restriction configured: every VLAN the switch knows.
    All,
    List(BTreeSet<u16>),
    /// Not configured in a grammar this understands.
    Unknown,
}

/// `switchport trunk allowed vlan …`, including the `add` lines IOS prints
/// when the list is too long for one line.
pub fn trunk_allowed(data: &DeviceData, interface: &str) -> Allowed {
    let Some(section) = ios_section(data, interface) else {
        return Allowed::Unknown;
    };
    const PREFIX: &str = "switchport trunk allowed vlan ";
    let mut vlans: Option<BTreeSet<u16>> = None;
    for child in &section.children {
        let Some(rest) = child.text.strip_prefix(PREFIX) else {
            continue;
        };
        let rest = rest.trim();
        if rest.eq_ignore_ascii_case("none") {
            vlans = Some(BTreeSet::new());
            continue;
        }
        let (list, adding) = match rest.strip_prefix("add ") {
            Some(list) => (list, true),
            None => (rest, false),
        };
        let Some(parsed) = parse_vlan_ranges(list) else {
            // A list this cannot read must not become "allows nothing".
            return Allowed::Unknown;
        };
        match (&mut vlans, adding) {
            (Some(existing), true) => existing.extend(parsed),
            (slot, _) => *slot = Some(parsed),
        }
    }
    match vlans {
        Some(vlans) => Allowed::List(vlans),
        // An IOS trunk with no `allowed` line carries everything.
        None => match data.config.as_ref().map(|config| config.syntax) {
            Some(Syntax::Ios) => Allowed::All,
            _ => Allowed::Unknown,
        },
    }
}

/// The native (untagged) VLAN of a trunk, from the collected value or the
/// configuration. An IOS-style trunk without a `native vlan` line is on 1.
pub fn native_vlan(data: &DeviceData, interface: &Interface) -> Option<u64> {
    if mode(data, interface) != Some(Mode::Trunk) {
        return None;
    }
    if let Some(vlan) = interface.vlan {
        return Some(vlan);
    }
    let section = ios_section(data, &interface.name)?;
    let configured = section.children.iter().find_map(|child| {
        child
            .text
            .strip_prefix("switchport trunk native vlan ")
            .and_then(|vlan| vlan.trim().parse::<u64>().ok())
    });
    Some(configured.unwrap_or(1))
}

/// Is the native VLAN written in the device's configuration, or is it 1
/// because nobody set it? The difference decides which end of a mismatch was
/// forgotten, and so which end a fix belongs on.
pub fn native_vlan_is_explicit(data: &DeviceData, interface: &str) -> bool {
    ios_section(data, interface).is_some_and(|section| {
        section
            .children
            .iter()
            .any(|child| child.text.starts_with("switchport trunk native vlan "))
    })
}

/// The VLAN of an access port, from the collected value or the configuration.
pub fn access_vlan(data: &DeviceData, interface: &Interface) -> Option<u64> {
    if mode(data, interface) != Some(Mode::Access) {
        return None;
    }
    if let Some(vlan) = interface.vlan {
        return Some(vlan);
    }
    let section = ios_section(data, &interface.name)?;
    let configured = section.children.iter().find_map(|child| {
        child
            .text
            .strip_prefix("switchport access vlan ")
            .and_then(|vlan| vlan.trim().parse::<u64>().ok())
    });
    // An access port with no `access vlan` line is in VLAN 1.
    Some(configured.unwrap_or(1))
}

/// Is this a physical port, as opposed to an SVI, a loopback or a tunnel?
/// Virtual interfaces have no duplex, no cable and no far end.
pub fn is_physical(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    ![
        "vlan",
        "loopback",
        "tunnel",
        "null",
        "bdi",
        "bvi",
        "port-channel",
    ]
    .iter()
    .any(|virtual_kind| lowered.starts_with(virtual_kind))
}

/// Default VLAN names that say nothing: `default`, `VLAN0010`, `VLAN_0010`.
pub fn is_default_vlan_name(id: u64, name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    if name.is_empty() || name == "default" {
        return true;
    }
    let digits: String = name
        .trim_start_matches("vlan")
        .trim_start_matches(['_', '-', ' '])
        .to_string();
    digits.parse::<u64>().is_ok_and(|parsed| parsed == id)
        && name.starts_with("vlan")
        && name.len() <= 9
}

/// A VLAN name reduced to what a difference in it would actually mean:
/// case, spaces, hyphens and underscores are not a difference.
pub fn vlan_name_key(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// A configuration's own `line vty` blocks.
pub fn vty_lines(config: &Config) -> Vec<&Node> {
    config
        .tree
        .roots
        .iter()
        .filter(|node| node.text.starts_with("line vty "))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdata::{device, iface, port};

    #[test]
    fn duplex_spellings_from_every_platform_we_have_seen() {
        for text in [
            "full",
            "Full",
            "FULL",
            "Full-duplex",
            "a-full",
            "auto-full",
            "full duplex",
        ] {
            assert_eq!(duplex(text), Duplex::Full, "{text}");
        }
        for text in ["half", "Half-duplex", "a-half", "HALF"] {
            assert_eq!(duplex(text), Duplex::Half, "{text}");
        }
        for text in ["", "auto", "Auto-duplex", "unknown", "yes", "no", "true"] {
            assert_eq!(duplex(text), Duplex::Unresolved, "{text}");
        }
    }

    #[test]
    fn speeds_read_like_a_person_wrote_them() {
        assert_eq!(speed(1_000_000_000), "1 Gb/s");
        assert_eq!(speed(100_000_000), "100 Mb/s");
        assert_eq!(speed(10_000_000_000), "10 Gb/s");
        assert_eq!(speed(1_500_000), "1.5 Mb/s");
        assert_eq!(speed(512), "512 b/s");
    }

    #[test]
    fn trunk_lists_come_out_of_the_configuration() {
        let data = device("a", "SW1")
            .interfaces(vec![iface("GigabitEthernet0/1"), iface("GigabitEthernet0/2")])
            .config(
                "interface GigabitEthernet0/1\n switchport mode trunk\n switchport trunk allowed vlan 10,20-22\n switchport trunk allowed vlan add 30\n\
                 interface GigabitEthernet0/2\n switchport mode trunk\n",
            );
        assert_eq!(
            trunk_allowed(&data, "Gi0/1"),
            Allowed::List([10, 20, 21, 22, 30].into_iter().collect())
        );
        assert_eq!(trunk_allowed(&data, "Gi0/2"), Allowed::All);
        assert_eq!(trunk_allowed(&data, "Gi0/9"), Allowed::Unknown);

        let none = device("a", "SW1")
            .interfaces(vec![iface("GigabitEthernet0/1")])
            .config("interface GigabitEthernet0/1\n switchport trunk allowed vlan none\n");
        assert_eq!(
            trunk_allowed(&none, "Gi0/1"),
            Allowed::List(BTreeSet::new())
        );

        // A configuration in a grammar we do not read says nothing at all.
        let routeros = device("a", "RB")
            .syntax(Syntax::Routeros)
            .interfaces(vec![iface("ether1")])
            .config("/interface bridge port add bridge=bridge1 interface=ether1\n");
        assert_eq!(trunk_allowed(&routeros, "ether1"), Allowed::Unknown);
    }

    #[test]
    fn native_and_access_vlans_fall_back_to_the_platform_default() {
        let data = device("a", "SW1")
            .interfaces(vec![
                iface("GigabitEthernet0/1"),
                iface("GigabitEthernet0/2"),
                iface("GigabitEthernet0/3"),
            ])
            .config(
                "interface GigabitEthernet0/1\n switchport mode trunk\n\
                 interface GigabitEthernet0/2\n switchport mode trunk\n switchport trunk native vlan 99\n\
                 interface GigabitEthernet0/3\n switchport mode access\n switchport access vlan 10\n",
            );
        let interfaces = data.interface_rows().to_vec();
        assert_eq!(native_vlan(&data, &interfaces[0]), Some(1));
        assert_eq!(native_vlan(&data, &interfaces[1]), Some(99));
        assert_eq!(native_vlan(&data, &interfaces[2]), None, "not a trunk");
        assert_eq!(access_vlan(&data, &interfaces[2]), Some(10));
        assert_eq!(mode(&data, &interfaces[0]), Some(Mode::Trunk));
    }

    #[test]
    fn collected_values_win_over_the_configuration() {
        let mut trunk = port("GigabitEthernet0/1", 1_000_000_000, "full");
        trunk.mode = "trunk".into();
        trunk.vlan = Some(50);
        let data = device("a", "SW1")
            .interfaces(vec![trunk.clone()])
            .config("interface GigabitEthernet0/1\n switchport trunk native vlan 99\n");
        assert_eq!(native_vlan(&data, &trunk), Some(50));
    }

    #[test]
    fn virtual_interfaces_and_default_vlan_names_are_recognised() {
        assert!(is_physical("GigabitEthernet0/1") && is_physical("ether1"));
        for name in ["Vlan10", "Loopback0", "Port-channel1", "Tunnel0", "Null0"] {
            assert!(!is_physical(name), "{name}");
        }
        assert!(is_default_vlan_name(10, "VLAN0010"));
        assert!(is_default_vlan_name(1, "default"));
        assert!(is_default_vlan_name(10, ""));
        assert!(!is_default_vlan_name(10, "users"));
        assert!(!is_default_vlan_name(10, "VLAN0020"));
        assert_eq!(vlan_name_key("Voice-VLAN"), vlan_name_key("voice_vlan"));
        assert_ne!(vlan_name_key("voice"), vlan_name_key("voip"));
    }
}

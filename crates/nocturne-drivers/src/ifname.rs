//! Interface-name normalisation.
//!
//! The same port is `Gi1/0/1` in a MAC table, `GigabitEthernet1/0/1` in the
//! running config and `gigabitethernet 1/0/1` when typed. Joining data from
//! different commands (and drawing links between devices) needs one spelling.

/// `(canonical long name, accepted short spellings)` for IOS-style platforms.
const KINDS: &[(&str, &[&str])] = &[
    (
        "GigabitEthernet",
        &["gi", "gig", "ge", "gigabitethernet", "gigabiteth"],
    ),
    ("FastEthernet", &["fa", "fas", "fastethernet", "fe"]),
    (
        "TenGigabitEthernet",
        &[
            "te",
            "ten",
            "tengigabitethernet",
            "tengige",
            "xe",
            "tengigabiteth",
        ],
    ),
    ("TwoGigabitEthernet", &["tw", "twogigabitethernet"]),
    ("FiveGigabitEthernet", &["fi", "fivegigabitethernet"]),
    (
        "TwentyFiveGigE",
        &["twe", "twentyfivegige", "twentyfivegigabitethernet"],
    ),
    (
        "FortyGigabitEthernet",
        &["fo", "fortygigabitethernet", "fortygige"],
    ),
    (
        "HundredGigE",
        &["hu", "hundredgige", "hundredgigabitethernet"],
    ),
    ("Ethernet", &["et", "eth", "ethernet"]),
    ("Port-channel", &["po", "port-channel", "portchannel"]),
    ("Vlan", &["vl", "vlan"]),
    ("Loopback", &["lo", "loopback"]),
    ("Tunnel", &["tu", "tunnel"]),
    ("Serial", &["se", "serial"]),
    ("Management", &["ma", "management"]),
    ("mgmt", &["mgmt"]),
    ("Null", &["nu", "null"]),
    ("BDI", &["bdi"]),
    ("BVI", &["bvi"]),
    ("AppGigabitEthernet", &["ap", "appgigabitethernet"]),
    ("Bundle-Ether", &["be", "bundle-ether"]),
];

/// Split `Gi1/0/1.100` into its alphabetic kind and the rest.
fn split(name: &str) -> (&str, &str) {
    let name = name.trim();
    let end = name
        .find(|c: char| c.is_ascii_digit())
        .unwrap_or(name.len());
    (name[..end].trim_end(), &name[end..])
}

/// The long form: `Gi1/0/1` → `GigabitEthernet1/0/1`. Unknown kinds are
/// returned unchanged (apart from removing a space before the number).
pub fn long(name: &str) -> String {
    let (kind, rest) = split(name);
    let lowered = kind.to_ascii_lowercase();
    match KINDS
        .iter()
        .find(|(_, aliases)| aliases.contains(&lowered.as_str()))
    {
        Some((canonical, _)) => format!("{canonical}{rest}"),
        None => format!("{kind}{rest}"),
    }
}

/// The conventional short form: `GigabitEthernet1/0/1` → `Gi1/0/1`.
pub fn short(name: &str) -> String {
    let (kind, rest) = split(name);
    let lowered = kind.to_ascii_lowercase();
    match KINDS
        .iter()
        .find(|(_, aliases)| aliases.contains(&lowered.as_str()))
    {
        Some((canonical, aliases)) => {
            // The first alias is the display abbreviation.
            let abbreviation = aliases.first().copied().unwrap_or(canonical);
            let mut chars = abbreviation.chars();
            let capitalised: String = match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            };
            format!("{capitalised}{rest}")
        }
        None => format!("{kind}{rest}"),
    }
}

/// Whether two spellings name the same interface.
pub fn same(a: &str, b: &str) -> bool {
    long(a).eq_ignore_ascii_case(&long(b))
}

/// A sort key that orders `Gi1/0/2` before `Gi1/0/10` and groups by kind.
pub fn sort_key(name: &str) -> (String, Vec<u64>) {
    let canonical = long(name);
    let (kind, rest) = split(&canonical);
    let numbers = rest
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(|part| part.parse::<u64>().unwrap_or(u64::MAX))
        .collect();
    (kind.to_ascii_lowercase(), numbers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_and_abbreviates() {
        for (given, expected) in [
            ("Gi1/0/1", "GigabitEthernet1/0/1"),
            ("gi 0/1", "GigabitEthernet0/1"),
            ("GigabitEthernet0/1", "GigabitEthernet0/1"),
            ("Fa0/24", "FastEthernet0/24"),
            ("Te1/1/4", "TenGigabitEthernet1/1/4"),
            ("Po12", "Port-channel12"),
            ("Vl99", "Vlan99"),
            ("lo0", "Loopback0"),
            ("Eth1/1", "Ethernet1/1"),
            ("Gi0/0.100", "GigabitEthernet0/0.100"),
            ("ether1", "ether1"),
            ("ge-0/0/1", "ge-0/0/1"),
            ("bridge1", "bridge1"),
        ] {
            assert_eq!(long(given), expected, "{given}");
        }
        assert_eq!(short("GigabitEthernet1/0/1"), "Gi1/0/1");
        assert_eq!(short("Port-channel5"), "Po5");
        assert_eq!(short("TenGigabitEthernet1/1/1"), "Te1/1/1");
        assert_eq!(short("ether1"), "ether1");
    }

    #[test]
    fn compares_and_sorts_naturally() {
        assert!(same("Gi0/1", "gigabitethernet0/1"));
        assert!(!same("Gi0/1", "Fa0/1"));
        let mut names = vec!["Gi1/0/10", "Gi1/0/2", "Fa0/1", "Gi1/0/1", "Vlan10", "Vlan2"];
        names.sort_by_key(|name| sort_key(name));
        assert_eq!(
            names,
            ["Fa0/1", "Gi1/0/1", "Gi1/0/2", "Gi1/0/10", "Vlan2", "Vlan10"]
        );
    }
}

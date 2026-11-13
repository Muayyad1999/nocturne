//! Interface names: long and short spellings, and natural sorting.
//!
//! The same port is `Gi1/0/1` in a MAC table, `GigabitEthernet1/0/1` in the
//! running configuration and `Gig 1/0/1` in `show cdp neighbors`. Only names
//! written the Cisco way (also used by Arista, and by Huawei and H3C for
//! `GE0/0/1`) are abbreviated like this; anything else — `ge-0/0/1`,
//! `ether1`, `eth0`, `lo0`, `bond0` — is passed through untouched.
//!
//! The long names are those of IOS and IOS XE. A few differ elsewhere
//! (`TenGigE` on IOS XR, `Port-Channel` on EOS); compare names through
//! [`interface_long_name`] or [`interface_sort_key`] rather than literally.

/// One kind of interface: its long name, its short name, and other
/// spellings that are accepted (the three-letter forms of `show cdp
/// neighbors`, IOS XR and Huawei variants). Matching ignores case.
struct Kind {
    long: &'static str,
    short: &'static str,
    aliases: &'static [&'static str],
}

const fn kind(long: &'static str, short: &'static str, aliases: &'static [&'static str]) -> Kind {
    Kind {
        long,
        short,
        aliases,
    }
}

const KINDS: &[Kind] = &[
    kind("Ethernet", "Eth", &["et"]),
    kind("FastEthernet", "Fa", &["fas"]),
    kind("GigabitEthernet", "Gi", &["gig", "ge", "gige"]),
    kind("TwoGigabitEthernet", "Tw", &["two"]),
    kind("FiveGigabitEthernet", "Fi", &["fiv"]),
    kind("TenGigabitEthernet", "Te", &["ten", "tengige"]),
    kind("TwentyFiveGigE", "Twe", &["twentyfivegigabitethernet"]),
    kind("FortyGigabitEthernet", "Fo", &["for", "fortygige"]),
    kind("HundredGigE", "Hu", &["hun", "hundredgigabitethernet"]),
    kind("FourHundredGigE", "FH", &["fou"]),
    kind("AppGigabitEthernet", "Ap", &[]),
    kind("Port-channel", "Po", &["portchannel"]),
    kind("Bundle-Ether", "BE", &[]),
    kind("Vlan", "Vl", &[]),
    kind("Loopback", "Lo", &[]),
    kind("Tunnel", "Tu", &[]),
    kind("Serial", "Se", &["ser"]),
    kind("Management", "Ma", &[]),
    kind("MgmtEth", "Mg", &[]),
    kind("Null", "Nu", &[]),
    kind("Dialer", "Di", &[]),
    kind("Virtual-Access", "Vi", &[]),
    kind("Virtual-Template", "Vt", &[]),
    kind("Multilink", "Mu", &[]),
    kind("Cellular", "Ce", &[]),
    kind("Vxlan", "Vx", &[]),
];

/// Lower-case names that are complete interface names on other systems
/// (Linux and SONiC `eth0`, Junos and BSD `lo0`), not abbreviations.
const NOT_ABBREVIATIONS: [&str; 2] = ["eth", "lo"];

/// Split `Gi1/0/1.100` (or `Gig 1/0/1`) into the letters before the first
/// digit and everything from that digit on, and find the kind those letters
/// name. `None` if the name does not have that shape or the letters are not
/// a known kind of interface.
fn parse(name: &str) -> Option<(&'static Kind, &str)> {
    let digit = name.find(|c: char| c.is_ascii_digit())?;
    let (letters, rest) = name.split_at(digit);
    let letters = letters.trim_end();
    // `Eth1/1` (NX-OS) has a slash that `eth0` never has.
    if NOT_ABBREVIATIONS.contains(&letters) && !rest.contains('/') {
        return None;
    }
    let found = KINDS.iter().find(|kind| {
        kind.long.eq_ignore_ascii_case(letters)
            || kind.short.eq_ignore_ascii_case(letters)
            || kind
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(letters))
    })?;
    Some((found, rest))
}

/// The long form of an interface name: `Gi1/0/1` → `GigabitEthernet1/0/1`.
///
/// Understands the abbreviations of IOS, IOS XE, NX-OS, IOS XR and EOS
/// (`Fa0/1`, `Te1/1/1`, `Twe1/0/1`, `Hu1/0/49`, `Eth1/1`, `Po1`, `Vl10`,
/// `Lo0`, `Tu0`, `Se0/0/0`, `Ma1`, `BE1` ...), the three-letter forms of
/// `show cdp neighbors` (`Gig 1/0/1`) and `GE0/0/1`, in any letter case.
/// Names that are not abbreviations of this kind are returned unchanged
/// (trimmed).
pub fn interface_long_name(short: &str) -> String {
    let name = short.trim();
    match parse(name) {
        Some((kind, rest)) => format!("{}{rest}", kind.long),
        None => name.to_owned(),
    }
}

/// The short form of an interface name: `GigabitEthernet1/0/1` → `Gi1/0/1`.
///
/// The inverse of [`interface_long_name`], with the same pass-through for
/// names it does not know.
pub fn interface_short_name(long: &str) -> String {
    let name = long.trim();
    match parse(name) {
        Some((kind, rest)) => format!("{}{rest}", kind.short),
        None => name.to_owned(),
    }
}

/// A key that sorts interface names the way people expect: `Gi1/0/2` before
/// `Gi1/0/10`, and `Gi1/0/1` together with `GigabitEthernet1/0/1`.
///
/// The (long, lower-cased) name is cut into pieces of text each followed by
/// a number: `Gi1/0/10` → `[("gigabitethernet", 1), ("/", 0), ("/", 10)]`.
/// Compare the keys with the ordinary ordering of `Vec`.
pub fn interface_sort_key(name: &str) -> Vec<(String, u64)> {
    /// Interface names are short; a key never needs more of one than this.
    const MAX_CHARS: usize = 256;
    let name: String = interface_long_name(name)
        .chars()
        .take(MAX_CHARS)
        .collect::<String>()
        .to_lowercase();
    let mut key = Vec::new();
    let mut rest = name.as_str();
    while !rest.is_empty() {
        let text_len = rest
            .find(|c: char| c.is_ascii_digit())
            .unwrap_or(rest.len());
        let (text, after_text) = rest.split_at(text_len);
        let digits_len = after_text
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after_text.len());
        let (digits, after_digits) = after_text.split_at(digits_len);
        // A number too long for u64 sorts after every number that fits.
        let number = if digits.is_empty() {
            0
        } else {
            digits.parse().unwrap_or(u64::MAX)
        };
        key.push((text.to_owned(), number));
        rest = after_digits;
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_names_expand_and_long_names_abbreviate() {
        for (short, long) in [
            ("Gi1/0/1", "GigabitEthernet1/0/1"),
            ("Te1/1/1", "TenGigabitEthernet1/1/1"),
            ("Po1", "Port-channel1"),
            ("Vl10", "Vlan10"),
            ("Lo0", "Loopback0"),
            ("Fa0/1", "FastEthernet0/1"),
            ("Eth1/1", "Ethernet1/1"),
            ("Hu1/0/1", "HundredGigE1/0/1"),
            ("Tw1/0/1", "TwoGigabitEthernet1/0/1"),
            ("Fi1/0/1", "FiveGigabitEthernet1/0/1"),
            ("Twe1/0/1", "TwentyFiveGigE1/0/1"),
            ("Fo1/1/1", "FortyGigabitEthernet1/1/1"),
            ("FH0/0/0/0", "FourHundredGigE0/0/0/0"),
            ("Tu100", "Tunnel100"),
            ("Se0/0/0:0", "Serial0/0/0:0"),
            ("Ma1", "Management1"),
            ("Mg0/RP0/CPU0/0", "MgmtEth0/RP0/CPU0/0"),
            ("BE10", "Bundle-Ether10"),
            ("Nu0", "Null0"),
            ("Gi0/0.100", "GigabitEthernet0/0.100"),
            ("Te1/0/1:1", "TenGigabitEthernet1/0/1:1"),
            ("Ap1/0/1", "AppGigabitEthernet1/0/1"),
            ("Di1", "Dialer1"),
            ("Vi2.1", "Virtual-Access2.1"),
            ("Vx1", "Vxlan1"),
        ] {
            assert_eq!(interface_long_name(short), long, "{short}");
            assert_eq!(interface_short_name(long), short, "{long}");
        }
    }

    #[test]
    fn other_spellings_are_understood() {
        for (given, long, short) in [
            // show cdp neighbors
            ("Gig 1/0/1", "GigabitEthernet1/0/1", "Gi1/0/1"),
            ("Fas 0/1", "FastEthernet0/1", "Fa0/1"),
            ("Ten 1/1/1", "TenGigabitEthernet1/1/1", "Te1/1/1"),
            ("For 1/1/1", "FortyGigabitEthernet1/1/1", "Fo1/1/1"),
            ("Hun 1/0/49", "HundredGigE1/0/49", "Hu1/0/49"),
            ("Two 1/0/1", "TwoGigabitEthernet1/0/1", "Tw1/0/1"),
            ("Ser 0/0/0", "Serial0/0/0", "Se0/0/0"),
            ("Eth 1/1", "Ethernet1/1", "Eth1/1"),
            // IOS and EOS abbreviate Ethernet to Et
            ("Et0/0", "Ethernet0/0", "Eth0/0"),
            ("Et1", "Ethernet1", "Eth1"),
            // typed by hand
            ("gi1/0/1", "GigabitEthernet1/0/1", "Gi1/0/1"),
            ("GI1/0/1", "GigabitEthernet1/0/1", "Gi1/0/1"),
            ("gigabitethernet 1/0/1", "GigabitEthernet1/0/1", "Gi1/0/1"),
            ("  Gi1/0/1  ", "GigabitEthernet1/0/1", "Gi1/0/1"),
            ("port-channel10", "Port-channel10", "Po10"),
            ("Port-Channel10", "Port-channel10", "Po10"),
            ("loopback0", "Loopback0", "Lo0"),
            ("LoopBack0", "Loopback0", "Lo0"),
            ("vlan 20", "Vlan20", "Vl20"),
            // IOS XR, IOS XE and Huawei variants
            ("TenGigE0/0/0/1", "TenGigabitEthernet0/0/0/1", "Te0/0/0/1"),
            ("HundredGigabitEthernet1/0/1", "HundredGigE1/0/1", "Hu1/0/1"),
            ("Fou1/0/1", "FourHundredGigE1/0/1", "FH1/0/1"),
            ("GE0/0/1", "GigabitEthernet0/0/1", "Gi0/0/1"),
        ] {
            assert_eq!(interface_long_name(given), long, "{given}");
            assert_eq!(interface_short_name(given), short, "{given}");
        }
    }

    #[test]
    fn long_names_abbreviate() {
        for (long, short) in [
            ("GigabitEthernet1/0/1", "Gi1/0/1"),
            ("TenGigabitEthernet1/1/1", "Te1/1/1"),
            ("FastEthernet0/24", "Fa0/24"),
            ("Ethernet1/1", "Eth1/1"),
            ("Port-channel5", "Po5"),
            ("port-channel5", "Po5"),
            ("Vlan99", "Vl99"),
            ("Loopback0", "Lo0"),
            ("TwentyFiveGigE1/0/1", "Twe1/0/1"),
            ("HundredGigE1/0/49", "Hu1/0/49"),
            ("FourHundredGigE0/0/0/0", "FH0/0/0/0"),
            ("Tunnel1", "Tu1"),
            ("Gi1/0/1", "Gi1/0/1"),
            ("gig 1/0/1", "Gi1/0/1"),
        ] {
            assert_eq!(interface_short_name(long), short, "{long}");
        }
    }

    #[test]
    fn names_of_other_platforms_pass_through() {
        for name in [
            "ge-0/0/1",
            "xe-0/0/0.0",
            "et-0/0/48",
            "ae0",
            "irb.100",
            "lo0.0",
            "ether1",
            "sfp-sfpplus1",
            "bridge1",
            "eth0",
            "eth1.100",
            "bond0",
            "swp1",
            "tun0",
            "wlan0",
            "mgmt0",
            "10GE1/0/1",
            "XGE0/0/1",
            "Eth-Trunk1",
            "Vlanif10",
            "BVI1",
            "ether",
            "Gi",
            "",
            "1/0/1",
            "?",
            "Gigabit",
            "unknown9",
        ] {
            assert_eq!(interface_long_name(name), name, "{name}");
            assert_eq!(interface_short_name(name), name, "{name}");
        }
        assert_eq!(interface_long_name("  ether1 "), "ether1");
        // NX-OS and Arista `Eth1/1` is an abbreviation; Linux `eth0` is a name.
        assert_eq!(interface_long_name("Eth1/1"), "Ethernet1/1");
        assert_eq!(interface_long_name("eth1/1"), "Ethernet1/1");
        assert_eq!(interface_long_name("Eth1"), "Ethernet1");
        assert_eq!(interface_long_name("eth1"), "eth1");
        // Likewise Cisco `Lo0` and Junos / BSD `lo0`.
        assert_eq!(interface_long_name("Lo0"), "Loopback0");
        assert_eq!(interface_long_name("lo0"), "lo0");
    }

    #[test]
    fn conversions_are_stable() {
        for name in [
            "Gi1/0/1",
            "GigabitEthernet1/0/1",
            "gig 1/0/1",
            "Po1",
            "Eth1/1",
            "ether1",
            "ge-0/0/0",
            "Twe1/0/1",
            "eth0",
        ] {
            let long = interface_long_name(name);
            let short = interface_short_name(name);
            assert_eq!(interface_long_name(&long), long, "{name}");
            assert_eq!(interface_short_name(&short), short, "{name}");
            assert_eq!(interface_long_name(&short), long, "{name}");
            assert_eq!(interface_short_name(&long), short, "{name}");
        }
    }

    #[test]
    fn every_kind_round_trips_and_is_unambiguous() {
        let mut spellings = std::collections::BTreeSet::new();
        for kind in KINDS {
            assert_eq!(
                interface_long_name(&format!("{}1", kind.short)),
                format!("{}1", kind.long)
            );
            assert_eq!(
                interface_short_name(&format!("{}1/2", kind.long)),
                format!("{}1/2", kind.short)
            );
            for spelling in [kind.long, kind.short].iter().chain(kind.aliases) {
                assert_eq!(
                    interface_long_name(&format!("{spelling}7/1")),
                    format!("{}7/1", kind.long),
                    "{spelling}"
                );
                assert!(
                    spellings.insert(spelling.to_ascii_lowercase()),
                    "`{spelling}` names two kinds"
                );
            }
        }
    }

    #[test]
    fn natural_sort() {
        let mut names = vec![
            "Gi1/0/10",
            "Gi1/0/2",
            "Fa0/1",
            "GigabitEthernet1/0/1",
            "Vlan10",
            "Vl2",
            "Gi1/0/2.100",
            "Gi1/0/2.20",
            "Te1/1/1",
            "Po2",
            "Po10",
            "Lo0",
        ];
        names.sort_by_key(|name| interface_sort_key(name));
        assert_eq!(
            names,
            [
                "Fa0/1",
                "GigabitEthernet1/0/1",
                "Gi1/0/2",
                "Gi1/0/2.20",
                "Gi1/0/2.100",
                "Gi1/0/10",
                "Lo0",
                "Po2",
                "Po10",
                "Te1/1/1",
                "Vl2",
                "Vlan10"
            ]
        );
        assert_eq!(
            interface_sort_key("Gi1/0/10"),
            [
                ("gigabitethernet".to_owned(), 1),
                ("/".to_owned(), 0),
                ("/".to_owned(), 10)
            ]
        );
        assert_eq!(
            interface_sort_key("Gi1/0/10"),
            interface_sort_key("GigabitEthernet1/0/10")
        );
        assert!(interface_sort_key("Gi1/0/2") < interface_sort_key("Gi1/0/10"));
        assert!(interface_sort_key("ge-0/0/2") < interface_sort_key("ge-0/0/10"));
        assert!(interface_sort_key("ether2") < interface_sort_key("ether10"));
    }

    #[test]
    fn sort_keys_of_odd_names() {
        assert_eq!(interface_sort_key(""), Vec::<(String, u64)>::new());
        assert_eq!(interface_sort_key("mgmt"), [("mgmt".to_owned(), 0)]);
        assert_eq!(
            interface_sort_key("10GE1/0/1"),
            [
                (String::new(), 10),
                ("ge".to_owned(), 1),
                ("/".to_owned(), 0),
                ("/".to_owned(), 1)
            ]
        );
        // Numbers beyond u64 saturate instead of failing.
        assert_eq!(
            interface_sort_key("x99999999999999999999999"),
            [("x".to_owned(), u64::MAX)]
        );
        assert!(
            interface_sort_key("x99999999999999999999999")
                > interface_sort_key("x18446744073709551614")
        );
        assert_eq!(interface_sort_key("É1"), [("é".to_owned(), 1)]);
        // Only the start of an absurdly long name goes into the key.
        assert_eq!(interface_sort_key(&"1/".repeat(100_000)).len(), 129);
    }
}

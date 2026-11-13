//! Administrative distance / route preference by vendor.

use serde::{Deserialize, Serialize};

/// How much each vendor trusts one source of routes by default (lower wins).
/// `None` where the platform does not have the source, or documents no default.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AdEntry {
    /// The route source, e.g. `OSPF (intra- and inter-area)`.
    pub source: String,
    /// Cisco IOS / IOS XE / NX-OS administrative distance.
    pub cisco: Option<u32>,
    /// Junos route preference.
    pub juniper: Option<u32>,
    /// MikroTik RouterOS distance.
    pub mikrotik: Option<u32>,
    /// Huawei VRP (external) preference.
    pub huawei: Option<u32>,
    /// Arista EOS administrative distance.
    pub arista: Option<u32>,
}

/// `(source, Cisco, Juniper, MikroTik, Huawei, Arista)`.
///
/// Sources: Cisco "What Is Administrative Distance?"; Junos "Default Route
/// Preference Values"; MikroTik RouterOS IP routing manual; Huawei "Routing
/// Protocol Preference" (the configurable external preference); Arista EOS
/// user manual (`distance` commands of each protocol).
#[rustfmt::skip]
const DISTANCES: &[(&str, Option<u32>, Option<u32>, Option<u32>, Option<u32>, Option<u32>)] = &[
    ("Connected",                      Some(0),   Some(0),   Some(0),   Some(0),   Some(0)),
    ("Static",                         Some(1),   Some(5),   Some(1),   Some(60),  Some(1)),
    ("EIGRP summary",                  Some(5),   None,      None,      None,      None),
    ("RSVP-TE LSP",                    None,      Some(7),   None,      None,      None),
    ("LDP LSP",                        None,      Some(9),   None,      None,      None),
    ("BGP external (eBGP)",            Some(20),  Some(170), Some(20),  Some(255), Some(200)),
    ("EIGRP internal",                 Some(90),  None,      None,      None,      None),
    ("OSPF (intra- and inter-area)",   Some(110), Some(10),  Some(110), Some(10),  Some(110)),
    ("IS-IS level 1",                  Some(115), Some(15),  None,      Some(15),  Some(115)),
    ("IS-IS level 2",                  Some(115), Some(18),  None,      Some(15),  Some(115)),
    ("RIP",                            Some(120), Some(100), Some(120), Some(100), Some(120)),
    ("MME (RouterOS mesh)",            None,      None,      Some(130), None,      None),
    ("Aggregate / generated",          None,      Some(130), None,      None,      None),
    ("OSPF external (AS external, NSSA)", Some(110), Some(150), Some(110), Some(150), Some(110)),
    ("IS-IS level 1 external",         Some(115), Some(160), None,      Some(15),  Some(115)),
    ("IS-IS level 2 external",         Some(115), Some(165), None,      Some(15),  Some(115)),
    ("EIGRP external",                 Some(170), None,      None,      None,      None),
    ("BGP internal (iBGP)",            Some(200), Some(170), Some(200), Some(255), Some(200)),
];

/// Default administrative distances (Cisco, Arista, MikroTik) and route
/// preferences (Juniper, Huawei) per route source.
///
/// These are the defaults a route of each source gets; every platform lets
/// them be changed. Lower values are preferred on all five.
pub fn administrative_distances() -> Vec<AdEntry> {
    DISTANCES
        .iter()
        .map(
            |(source, cisco, juniper, mikrotik, huawei, arista)| AdEntry {
                source: (*source).to_owned(),
                cisco: *cisco,
                juniper: *juniper,
                mikrotik: *mikrotik,
                huawei: *huawei,
                arista: *arista,
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(source: &str) -> AdEntry {
        administrative_distances()
            .into_iter()
            .find(|entry| entry.source.starts_with(source))
            .unwrap()
    }

    #[test]
    fn cisco_distances() {
        for (source, distance) in [
            ("Connected", 0),
            ("Static", 1),
            ("EIGRP summary", 5),
            ("BGP external", 20),
            ("EIGRP internal", 90),
            ("OSPF (", 110),
            ("IS-IS level 1", 115),
            ("RIP", 120),
            ("EIGRP external", 170),
            ("BGP internal", 200),
        ] {
            assert_eq!(row(source).cisco, Some(distance), "{source}");
        }
    }

    #[test]
    fn juniper_preferences() {
        for (source, preference) in [
            ("Connected", 0),
            ("Static", 5),
            ("RSVP", 7),
            ("LDP", 9),
            ("OSPF (", 10),
            ("IS-IS level 1", 15),
            ("IS-IS level 2", 18),
            ("RIP", 100),
            ("Aggregate", 130),
            ("OSPF external", 150),
            ("IS-IS level 1 external", 160),
            ("IS-IS level 2 external", 165),
            ("BGP external", 170),
            ("BGP internal", 170),
        ] {
            assert_eq!(row(source).juniper, Some(preference), "{source}");
        }
    }

    #[test]
    fn mikrotik_huawei_and_arista() {
        for (source, mikrotik, huawei, arista) in [
            ("Connected", Some(0), Some(0), Some(0)),
            ("Static", Some(1), Some(60), Some(1)),
            ("BGP external", Some(20), Some(255), Some(200)),
            ("OSPF (", Some(110), Some(10), Some(110)),
            ("IS-IS level 1", None, Some(15), Some(115)),
            ("RIP", Some(120), Some(100), Some(120)),
            ("MME", Some(130), None, None),
            ("OSPF external", Some(110), Some(150), Some(110)),
            ("BGP internal", Some(200), Some(255), Some(200)),
        ] {
            let entry = row(source);
            assert_eq!(
                (entry.mikrotik, entry.huawei, entry.arista),
                (mikrotik, huawei, arista),
                "{source}"
            );
        }
    }

    #[test]
    fn every_row_has_a_value_and_a_unique_source() {
        let table = administrative_distances();
        assert_eq!(table.len(), 18);
        for entry in &table {
            let values = [
                entry.cisco,
                entry.juniper,
                entry.mikrotik,
                entry.huawei,
                entry.arista,
            ];
            assert!(values.iter().any(Option::is_some), "{}", entry.source);
            assert!(
                values.iter().flatten().all(|value| *value <= 255),
                "{}",
                entry.source
            );
            assert_eq!(
                table
                    .iter()
                    .filter(|other| other.source == entry.source)
                    .count(),
                1
            );
        }
    }
}

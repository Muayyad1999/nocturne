//! A built-in dictionary of the MIB objects network engineers meet daily; no MIB compiler.
//!
//! About 3 800 objects (the common IETF/IEEE MIBs plus the health, inventory and neighbour
//! objects of the usual vendors) and 1 250 notifications, each taken from its defining MIB
//! module. The objects that also have a constant are listed in [`crate::oids`].
//!
//! ```
//! use nocturne_snmp::{Oid, names};
//!
//! let oid: Oid = "1.3.6.1.2.1.2.2.1.10.3".parse().unwrap();
//! assert_eq!(names::describe(&oid).as_deref(), Some("ifInOctets.3"));
//! assert_eq!(names::resolve("IF-MIB::ifInOctets.3"), Some(oid));
//!
//! let link_down: Oid = "1.3.6.1.6.3.1.1.5.3".parse().unwrap();
//! assert_eq!(names::trap_name(&link_down), Some("linkDown"));
//! ```

mod extra;

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::Oid;

/// One named object.
struct Object {
    module: &'static str,
    name: &'static str,
    /// Another object of the same name was defined first (in a more common MIB), so this
    /// one is only reachable — and shown — with its module.
    shadowed: bool,
}

struct Dictionary {
    objects: HashMap<Vec<u32>, Object>,
    by_name: HashMap<&'static str, Oid>,
    by_qualified_name: HashMap<(&'static str, &'static str), Oid>,
    /// Lower-cased names, for forgiving lookups.
    by_folded_name: HashMap<String, Oid>,
    traps: HashMap<Vec<u32>, &'static str>,
    trap_by_name: HashMap<&'static str, Oid>,
    /// Length of the longest object OID: no prefix longer than this can match.
    longest: usize,
}

static DICTIONARY: LazyLock<Dictionary> = LazyLock::new(Dictionary::build);

impl Dictionary {
    fn build() -> Dictionary {
        let mut dictionary = Dictionary {
            objects: HashMap::new(),
            by_name: HashMap::new(),
            by_qualified_name: HashMap::new(),
            by_folded_name: HashMap::new(),
            traps: HashMap::new(),
            trap_by_name: HashMap::new(),
            longest: 0,
        };
        let rows = crate::oids::OBJECTS.iter().chain(extra::OBJECTS);
        for &(module, name, oid) in rows {
            // The tables are static data checked by the tests; a bad row is skipped, not fatal.
            let Ok(oid) = oid.parse::<Oid>() else {
                continue;
            };
            let shadowed = dictionary.by_name.contains_key(name);
            if !shadowed {
                dictionary.by_name.insert(name, oid.clone());
                dictionary
                    .by_folded_name
                    .entry(name.to_ascii_lowercase())
                    .or_insert_with(|| oid.clone());
            }
            dictionary
                .by_qualified_name
                .insert((module, name), oid.clone());
            dictionary.longest = dictionary.longest.max(oid.len());
            dictionary
                .objects
                .entry(oid.parts().to_vec())
                .or_insert(Object {
                    module,
                    name,
                    shadowed,
                });
        }
        for &(_module, name, oid) in extra::TRAPS {
            let Ok(oid) = oid.parse::<Oid>() else {
                continue;
            };
            dictionary.traps.entry(oid.parts().to_vec()).or_insert(name);
            dictionary.trap_by_name.entry(name).or_insert(oid);
        }
        dictionary
    }

    /// The object whose OID is the longest prefix of `oid`, and what is left of `oid`.
    fn lookup<'a>(&self, oid: &'a Oid) -> Option<(&Object, &'a [u32])> {
        let parts = oid.parts();
        (1..=parts.len().min(self.longest)).rev().find_map(|len| {
            let (prefix, suffix) = parts.split_at(len);
            self.objects.get(prefix).map(|object| (object, suffix))
        })
    }
}

/// "1.3.6.1.2.1.2.2.1.10.3" → Some("ifInOctets.3")
///
/// The name of the deepest known object `oid` lies under, followed by the rest of the OID
/// (the instance: `.0` for scalars, the table index for columns). Notification OIDs — the
/// value of `snmpTrapOID.0` — are named too. `None` if nothing in the dictionary matches.
pub fn describe(oid: &super::Oid) -> Option<String> {
    let dictionary = &*DICTIONARY;
    // A notification is identified by its exact OID; objects by the longest known prefix.
    if let Some(name) = dictionary.traps.get(oid.parts()) {
        return Some((*name).to_owned());
    }
    let (object, suffix) = dictionary.lookup(oid)?;
    let mut text = if object.shadowed {
        format!("{}::{}", object.module, object.name)
    } else {
        object.name.to_owned()
    };
    for part in suffix {
        text.push('.');
        text.push_str(&part.to_string());
    }
    Some(text)
}

/// "ifInOctets" / "IF-MIB::ifInOctets" / "sysName.0" → Oid
///
/// Accepts an object name with an optional `MODULE::` qualifier and an optional numeric
/// instance suffix, a notification name (`linkDown`), or a plain dotted OID. Names are
/// matched exactly first and then without regard to case. The qualifier only matters for the
/// few names that exist in more than one MIB; otherwise it is not checked.
pub fn resolve(name: &str) -> Option<super::Oid> {
    let name = name.trim();
    if let Ok(oid) = name.parse::<Oid>() {
        return Some(oid);
    }
    let (module, rest) = match name.split_once("::") {
        Some((module, rest)) => (Some(module), rest),
        None => (None, name),
    };
    let (base, suffix) = match rest.split_once('.') {
        Some((base, suffix)) => (base, Some(suffix)),
        None => (rest, None),
    };
    let suffix: Oid = match suffix {
        // (`Oid` itself tolerates a leading dot; "ifDescr..3" is still a typo.)
        Some(suffix) if !suffix.starts_with('.') => suffix.parse().ok()?,
        Some(_) => return None,
        None => Oid::default(),
    };

    let dictionary = &*DICTIONARY;
    let object = module
        .and_then(|module| dictionary.by_qualified_name.get(&(module, base)))
        .or_else(|| dictionary.by_name.get(base))
        .or_else(|| dictionary.trap_by_name.get(base))
        .or_else(|| dictionary.by_folded_name.get(&base.to_ascii_lowercase()))?;
    Some(object.join(suffix.parts()))
}

/// Well-known trap OIDs → short human name ("linkDown", "coldStart", "bgpBackwardTransition"…)
///
/// SNMPv1 traps are covered through their RFC 3584 translation, which is what
/// [`Trap::trap_oid`](crate::Trap) holds.
pub fn trap_name(oid: &super::Oid) -> Option<&'static str> {
    DICTIONARY.traps.get(oid.parts()).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oids;
    use pretty_assertions::assert_eq;
    use std::collections::HashSet;

    fn oid(s: &str) -> Oid {
        s.parse().unwrap()
    }

    #[test]
    fn contract_examples() {
        assert_eq!(
            describe(&oid("1.3.6.1.2.1.2.2.1.10.3")).as_deref(),
            Some("ifInOctets.3")
        );
        assert_eq!(resolve("ifInOctets"), Some(oid("1.3.6.1.2.1.2.2.1.10")));
        assert_eq!(
            resolve("IF-MIB::ifInOctets"),
            Some(oid("1.3.6.1.2.1.2.2.1.10"))
        );
        assert_eq!(resolve("sysName.0"), Some(oid("1.3.6.1.2.1.1.5.0")));
        assert_eq!(trap_name(&oid("1.3.6.1.6.3.1.1.5.3")), Some("linkDown"));
        assert_eq!(trap_name(&oid("1.3.6.1.6.3.1.1.5.1")), Some("coldStart"));
        assert_eq!(
            trap_name(&oid("1.3.6.1.2.1.15.7.2")),
            Some("bgpBackwardTransition")
        );
    }

    #[test]
    fn describe_names_the_deepest_object() {
        let cases = [
            ("1.3.6.1.2.1.1.1.0", "sysDescr.0"),
            ("1.3.6.1.2.1.1.3.0", "sysUpTime.0"),
            ("1.3.6.1.2.1.1.5", "sysName"),
            ("1.3.6.1.2.1.2.2", "ifTable"),
            ("1.3.6.1.2.1.2.2.1", "ifEntry"),
            ("1.3.6.1.2.1.2.2.1.2.10001", "ifDescr.10001"),
            ("1.3.6.1.2.1.31.1.1.1.6.3", "ifHCInOctets.3"),
            ("1.3.6.1.2.1.31.1.1.1.18.7", "ifAlias.7"),
            (
                "1.3.6.1.2.1.4.22.1.2.2.10.0.0.1",
                "ipNetToMediaPhysAddress.2.10.0.0.1",
            ),
            ("1.3.6.1.2.1.25.3.3.1.2.196608", "hrProcessorLoad.196608"),
            ("1.3.6.1.2.1.25.2.1.2", "hrStorageRam"),
            ("1.3.6.1.2.1.47.1.1.1.1.11.1", "entPhysicalSerialNum.1"),
            ("1.3.6.1.2.1.99.1.1.1.4.1005", "entPhySensorValue.1005"),
            (
                "1.3.6.1.2.1.17.4.3.1.2.0.27.33.60.157.248",
                "dot1dTpFdbPort.0.27.33.60.157.248",
            ),
            ("1.3.6.1.2.1.17.7.1.4.3.1.1.100", "dot1qVlanStaticName.100"),
            ("1.0.8802.1.1.2.1.4.1.1.9.0.5.1", "lldpRemSysName.0.5.1"),
            ("1.3.6.1.2.1.15.3.1.2.192.0.2.1", "bgpPeerState.192.0.2.1"),
            ("1.3.6.1.4.1.2021.11.53.0", "ssCpuRawIdle.0"),
            ("1.3.6.1.4.1.2021.4.5.0", "memTotalReal.0"),
            ("1.3.6.1.4.1.9.9.109.1.1.1.1.8.1", "cpmCPUTotal5minRev.1"),
            ("1.3.6.1.4.1.9.9.48.1.1.1.5.1", "ciscoMemoryPoolUsed.1"),
            (
                "1.3.6.1.4.1.9.9.13.1.3.1.3.1",
                "ciscoEnvMonTemperatureStatusValue.1",
            ),
            ("1.3.6.1.4.1.14988.1.1.3.10.0", "mtxrHlTemperature.0"),
            (
                "1.3.6.1.4.1.2636.3.1.13.1.8.9.1.0.0",
                "jnxOperatingCPU.9.1.0.0",
            ),
            ("1.3.6.1.4.1.2011.5.25.31.1.1.1.1.5.9", "hwEntityCpuUsage.9"),
            ("1.3.6.1.4.1.11.2.14.11.5.1.9.6.1.0", "hpSwitchCpuStat.0"),
            (
                "1.3.6.1.4.1.25506.2.6.1.1.1.1.6.212",
                "hh3cEntityExtCpuUsage.212",
            ),
            ("1.3.6.1.4.1.12356.101.4.1.3.0", "fgSysCpuUsage.0"),
            ("1.3.6.1.4.1.25461.2.1.2.3.3.0", "panSessionActive.0"),
            ("1.3.6.1.4.1.10002.1.1.1.1.1.0", "memTotal.0"),
            (
                "1.3.6.1.4.1.30065.3.12.1.1.1.1.100",
                "aristaEntSensorThresholdLowWarning.100",
            ),
            ("1.3.6.1.6.3.15.1.1.5.0", "usmStatsWrongDigests.0"),
            ("1.3.6.1.6.3.1.1.4.1.0", "snmpTrapOID.0"),
            // Notification OIDs, as found in the value of snmpTrapOID.0.
            ("1.3.6.1.6.3.1.1.5.4", "linkUp"),
            ("1.3.6.1.4.1.9.9.43.2.0.1", "ciscoConfigManEvent"),
        ];
        for (numeric, name) in cases {
            assert_eq!(describe(&oid(numeric)).as_deref(), Some(name), "{numeric}");
        }
        for unknown in ["1.3.6.1.4.1.99999.1.2.3", "1.3", "0.0", "2.999.1"] {
            assert_eq!(describe(&oid(unknown)), None, "{unknown}");
        }
        assert_eq!(describe(&Oid::default()), None);
    }

    #[test]
    fn resolve_accepts_the_usual_spellings() {
        let if_in_octets_3 = oid("1.3.6.1.2.1.2.2.1.10.3");
        for spelling in [
            "ifInOctets.3",
            "IF-MIB::ifInOctets.3",
            " ifInOctets.3 ",
            "ifinoctets.3",
            "IFINOCTETS.3",
            // The module is not checked for unambiguous names (net-snmp prints RFC1213-MIB
            // or IF-MIB depending on what it has loaded).
            "RFC1213-MIB::ifInOctets.3",
            "1.3.6.1.2.1.2.2.1.10.3",
            ".1.3.6.1.2.1.2.2.1.10.3",
        ] {
            assert_eq!(
                resolve(spelling),
                Some(if_in_octets_3.clone()),
                "{spelling:?}"
            );
        }
        assert_eq!(
            resolve("ipNetToMediaPhysAddress.2.10.0.0.1"),
            Some(oid("1.3.6.1.2.1.4.22.1.2.2.10.0.0.1"))
        );
        assert_eq!(resolve("ifTable"), Some(oid("1.3.6.1.2.1.2.2")));
        assert_eq!(resolve("hrStorageRam"), Some(oid("1.3.6.1.2.1.25.2.1.2")));
        assert_eq!(resolve("linkDown"), Some(oid("1.3.6.1.6.3.1.1.5.3")));
        assert_eq!(resolve("IF-MIB::linkUp"), Some(oid("1.3.6.1.6.3.1.1.5.4")));
        assert_eq!(
            resolve("bgpBackwardTransition"),
            Some(oid("1.3.6.1.2.1.15.7.2"))
        );

        for unknown in [
            "",
            " ",
            "noSuchObjectName",
            "ifInOctets.",
            "ifInOctets.x",
            "ifInOctets..3",
            "IF-MIB::",
            "::ifInOctets.x",
            ".",
            "1.3.6.x",
        ] {
            assert_eq!(resolve(unknown), None, "{unknown:?}");
        }
    }

    #[test]
    fn names_shared_by_two_mibs_need_the_module() {
        // IF-MIB and UCD-SNMP-MIB own the plain names...
        assert_eq!(resolve("ifDescr"), Some(oid("1.3.6.1.2.1.2.2.1.2")));
        assert_eq!(resolve("memBuffer.0"), Some(oid("1.3.6.1.4.1.2021.4.14.0")));
        assert_eq!(resolve("ifIndex.7"), Some(oid("1.3.6.1.2.1.2.2.1.1.7")));
        // ...Palo Alto's and Ubiquiti's namesakes are reached with their module...
        let pan = oid("1.3.6.1.4.1.25461.2.1.2.3.11.1.2");
        assert_eq!(resolve("PAN-COMMON-MIB::ifDescr"), Some(pan.clone()));
        assert_eq!(pan.to_string(), oids::PAN_IF_DESCR);
        assert_eq!(
            resolve("PAN-COMMON-MIB::ifIndex"),
            Some(oid(oids::PAN_IF_INDEX))
        );
        let frogfoot = oid(oids::FROGFOOT_MEM_BUFFER);
        assert_eq!(
            resolve("FROGFOOT-RESOURCES-MIB::memBuffer.0"),
            Some(frogfoot.clone())
        );
        // ...and described with it, so that the two cannot be confused.
        assert_eq!(
            describe(&pan.child(4)).as_deref(),
            Some("PAN-COMMON-MIB::ifDescr.4")
        );
        assert_eq!(
            describe(&frogfoot).as_deref(),
            Some("FROGFOOT-RESOURCES-MIB::memBuffer.0")
        );
        assert_eq!(
            describe(&oid("1.3.6.1.2.1.2.2.1.2.4")).as_deref(),
            Some("ifDescr.4")
        );
    }

    #[test]
    fn every_object_round_trips() {
        let mut seen_oids = HashSet::new();
        let mut count = 0;
        for &(module, name, numeric) in oids::OBJECTS.iter().chain(extra::OBJECTS) {
            count += 1;
            let object: Oid = numeric
                .parse()
                .unwrap_or_else(|e| panic!("{module}::{name}: {e}"));
            assert!(crate::ber::check_oid(&object).is_ok(), "{module}::{name}");
            assert!(
                seen_oids.insert(numeric),
                "duplicate OID {numeric} ({module}::{name})"
            );
            assert!(
                name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                    && name.starts_with(|c: char| c.is_ascii_lowercase()),
                "odd object name {name:?}"
            );

            // Qualified, the name always resolves to its own OID; the instance is kept.
            let qualified = format!("{module}::{name}");
            assert_eq!(resolve(&qualified).as_ref(), Some(&object), "{qualified}");
            assert_eq!(
                resolve(&format!("{qualified}.7.42")),
                Some(object.join(&[7, 42])),
                "{qualified}"
            );

            // describe() gives a name that resolves back to the same OID.
            let described =
                describe(&object).unwrap_or_else(|| panic!("{qualified} is not described"));
            assert!(
                described == name || described == qualified,
                "{numeric} is described as {described:?}, expected {name:?}"
            );
            assert_eq!(resolve(&described).as_ref(), Some(&object), "{described}");
            // (A sub-identifier no MIB uses, so that no named child gets in the way.)
            let instance = object.join(&[u32::MAX, 0]);
            let described = describe(&instance).unwrap();
            assert!(described.ends_with(".4294967295.0"), "{described}");
            assert_eq!(resolve(&described), Some(instance), "{described}");
        }
        assert!(count > 3_500, "the dictionary shrank to {count} objects");
    }

    #[test]
    fn every_trap_round_trips() {
        let mut seen_oids = HashSet::new();
        let mut seen_names = HashSet::new();
        for &(module, name, numeric) in extra::TRAPS {
            let trap: Oid = numeric
                .parse()
                .unwrap_or_else(|e| panic!("{module}::{name}: {e}"));
            assert!(seen_oids.insert(numeric), "duplicate trap OID {numeric}");
            assert!(seen_names.insert(name), "duplicate trap name {name}");
            assert_eq!(trap_name(&trap), Some(name), "{numeric}");
            // Even with a named node above it (`bgpTraps`), the notification wins.
            assert_eq!(describe(&trap).as_deref(), Some(name), "{numeric}");
            assert_eq!(resolve(&format!("{module}::{name}")), Some(trap), "{name}");
        }
        assert!(extra::TRAPS.len() > 1_000);
        assert_eq!(trap_name(&oid("1.3.6.1.6.3.1.1.5")), None);
        assert_eq!(trap_name(&oid("1.3.6.1.2.1.1.3.0")), None);
    }

    #[test]
    fn the_snmpv2_generic_traps_are_all_there() {
        let generic = [
            "coldStart",
            "warmStart",
            "linkDown",
            "linkUp",
            "authenticationFailure",
            "egpNeighborLoss",
        ];
        for (number, name) in (1u32..).zip(generic) {
            let trap = oid("1.3.6.1.6.3.1.1.5").child(number);
            assert_eq!(trap_name(&trap), Some(name));
        }
        let well_known = [
            ("1.3.6.1.2.1.15.7.1", "bgpEstablished"),
            ("1.3.6.1.2.1.14.16.2.2", "ospfNbrStateChange"),
            ("1.3.6.1.2.1.17.0.1", "newRoot"),
            ("1.3.6.1.2.1.17.0.2", "topologyChange"),
            ("1.3.6.1.2.1.47.2.0.1", "entConfigChange"),
            ("1.0.8802.1.1.2.0.0.1", "lldpRemTablesChange"),
            ("1.3.6.1.4.1.9.9.43.2.0.1", "ciscoConfigManEvent"),
            ("1.3.6.1.4.1.8072.4.0.2", "nsNotifyShutdown"),
        ];
        for (numeric, name) in well_known {
            assert_eq!(trap_name(&oid(numeric)), Some(name), "{numeric}");
        }
    }

    #[test]
    fn constants_agree_with_the_dictionary() {
        // Scalars carry their instance, columns and tables do not.
        assert_eq!(oids::SYS_DESCR, "1.3.6.1.2.1.1.1.0");
        assert_eq!(oids::SYS_UP_TIME, "1.3.6.1.2.1.1.3.0");
        assert_eq!(oids::SYS_NAME, "1.3.6.1.2.1.1.5.0");
        assert_eq!(oids::IF_NUMBER, "1.3.6.1.2.1.2.1.0");
        assert_eq!(oids::IF_TABLE, "1.3.6.1.2.1.2.2");
        assert_eq!(oids::IF_INDEX, "1.3.6.1.2.1.2.2.1.1");
        assert_eq!(oids::IF_DESCR, "1.3.6.1.2.1.2.2.1.2");
        assert_eq!(oids::IF_OPER_STATUS, "1.3.6.1.2.1.2.2.1.8");
        assert_eq!(oids::IF_IN_OCTETS, "1.3.6.1.2.1.2.2.1.10");
        assert_eq!(oids::IF_NAME, "1.3.6.1.2.1.31.1.1.1.1");
        assert_eq!(oids::IF_HC_IN_OCTETS, "1.3.6.1.2.1.31.1.1.1.6");
        assert_eq!(oids::IF_HC_OUT_OCTETS, "1.3.6.1.2.1.31.1.1.1.10");
        assert_eq!(oids::IF_HIGH_SPEED, "1.3.6.1.2.1.31.1.1.1.15");
        assert_eq!(oids::IF_ALIAS, "1.3.6.1.2.1.31.1.1.1.18");
        assert_eq!(oids::IP_NET_TO_MEDIA_PHYS_ADDRESS, "1.3.6.1.2.1.4.22.1.2");
        assert_eq!(oids::HR_SYSTEM_UPTIME, "1.3.6.1.2.1.25.1.1.0");
        assert_eq!(oids::HR_PROCESSOR_LOAD, "1.3.6.1.2.1.25.3.3.1.2");
        assert_eq!(oids::HR_STORAGE_USED, "1.3.6.1.2.1.25.2.3.1.6");
        assert_eq!(oids::ENT_PHYSICAL_SERIAL_NUM, "1.3.6.1.2.1.47.1.1.1.1.11");
        assert_eq!(oids::ENT_PHY_SENSOR_VALUE, "1.3.6.1.2.1.99.1.1.1.4");
        assert_eq!(oids::DOT1D_TP_FDB_PORT, "1.3.6.1.2.1.17.4.3.1.2");
        assert_eq!(oids::DOT1Q_TP_FDB_PORT, "1.3.6.1.2.1.17.7.1.2.2.1.2");
        assert_eq!(oids::LLDP_REM_SYS_NAME, "1.0.8802.1.1.2.1.4.1.1.9");
        assert_eq!(oids::LLDP_LOC_SYS_NAME, "1.0.8802.1.1.2.1.3.3.0");
        assert_eq!(oids::SS_CPU_IDLE, "1.3.6.1.4.1.2021.11.11.0");
        assert_eq!(oids::MEM_TOTAL_REAL, "1.3.6.1.4.1.2021.4.5.0");
        assert_eq!(oids::LA_LOAD, "1.3.6.1.4.1.2021.10.1.3");
        assert_eq!(oids::CPM_CPU_TOTAL5MIN_REV, "1.3.6.1.4.1.9.9.109.1.1.1.1.8");
        assert_eq!(oids::CISCO_MEMORY_POOL_USED, "1.3.6.1.4.1.9.9.48.1.1.1.5");
        assert_eq!(oids::MTXR_HL_TEMPERATURE, "1.3.6.1.4.1.14988.1.1.3.10.0");
        assert_eq!(oids::JNX_OPERATING_CPU, "1.3.6.1.4.1.2636.3.1.13.1.8");
        assert_eq!(
            oids::HW_ENTITY_CPU_USAGE,
            "1.3.6.1.4.1.2011.5.25.31.1.1.1.1.5"
        );
        assert_eq!(
            oids::HP_SWITCH_CPU_STAT,
            "1.3.6.1.4.1.11.2.14.11.5.1.9.6.1.0"
        );
        assert_eq!(oids::FG_SYS_CPU_USAGE, "1.3.6.1.4.1.12356.101.4.1.3.0");
        assert_eq!(oids::PAN_SESSION_ACTIVE, "1.3.6.1.4.1.25461.2.1.2.3.3.0");

        // And every constant's object is in the dictionary under its own name.
        assert!(oids::OBJECTS.len() > 900);
        for &(module, name, numeric) in oids::OBJECTS {
            assert_eq!(
                resolve(&format!("{module}::{name}")),
                Some(oid(numeric)),
                "{module}::{name}"
            );
        }
    }
}

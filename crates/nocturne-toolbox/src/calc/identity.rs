//! Identifiers: MAC addresses and the IEEE registry, SNMP object names and
//! interface names.
//!
//! `tools.mac` and the prefix half of `tools.oui` are `nocturne_tools::mac`.
//! The name half of `tools.oui` is explained where it is implemented: the
//! engine can look a prefix up but cannot search its registry, and this crate
//! will not keep a second copy of the registry to work around that.

use std::sync::OnceLock;

use nocturne_app::{App, AppError, AppResult, Router, util};
use nocturne_snmp::{Oid, names};
use nocturne_tools::{mac, reference};
use serde::{Deserialize, Serialize};

use crate::errors::tool;

#[derive(Debug, Deserialize)]
pub struct InputParams {
    pub input: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct OuiParams {
    /// A MAC address, an OUI prefix (`00:1b:21`), or part of a vendor's name.
    pub query: String,
    pub limit: Option<usize>,
}

/// What the bundled IEEE registry is, so a stale answer can be recognised as one.
#[derive(Debug, Serialize)]
pub struct RegistryInfo {
    pub assignments: usize,
    /// The day the registry was downloaded from the IEEE (`YYYY-MM-DD`).
    pub date: String,
}

#[derive(Debug, Serialize)]
pub struct OuiResult {
    pub query: String,
    /// How the query was read: `prefix` or `vendor`.
    pub read_as: &'static str,
    pub assignments: Vec<Assignment>,
    /// Matches found, before `limit`.
    pub total: usize,
    pub registry: RegistryInfo,
    /// What this answer does not cover, when that is worth saying.
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Assignment {
    /// `00:1B:21`.
    pub prefix: String,
    pub vendor: String,
}

#[derive(Debug, Serialize)]
pub struct OidResult {
    pub input: String,
    /// The numeric OID, when the input named something in the dictionary.
    pub oid: Option<String>,
    /// `ifInOctets.3` — the object and the instance.
    pub name: Option<String>,
    /// The short name of the notification, when the OID is one.
    pub notification: Option<String>,
    /// `false` when nothing in the built-in dictionary matches; there is no
    /// MIB compiler here, so that is an answer and not a failure.
    pub known: bool,
}

#[derive(Debug, Serialize)]
pub struct InterfaceResult {
    pub long: String,
    pub short: String,
}

/// The longest vendor-name search result returned in one answer.
const OUI_LIMIT: usize = 100;

/// The bundled registry, indexed by vendor instead of by prefix.
///
/// `nocturne_tools::mac` exposes the registry as a lookup — prefix in, name
/// out — and nothing else, so "which blocks does Ubiquiti own?" cannot be
/// asked of it. Rather than keep a second copy of the IEEE registry in this
/// crate (data duplication, and it would go stale separately), the index is
/// derived from the engine itself: every prefix the IEEE could have assigned
/// is looked up once and the hits are kept.
///
/// "Could have assigned" is what keeps this affordable. An assignment's first
/// octet always has the multicast and the locally-administered bit clear —
/// those two bits belong to whoever *uses* an address, not to the block — so
/// three quarters of the 16.7 M prefixes cannot be assignments and are not
/// looked up. The remaining 4.2 M take about a third of a second of one core,
/// once per process, only when somebody actually searches by name, and on a
/// blocking thread rather than a runtime worker.
///
/// It finds the 24-bit MA-L assignments only; the longer MA-M, MA-S and IAB
/// blocks are reachable by prefix but not by name, and the answer says so
/// instead of pretending the list is complete. A `mac::search(name)` in the
/// engine would replace all of this, and is in the report.
fn vendor_index() -> &'static Vec<Assignment> {
    static INDEX: OnceLock<Vec<Assignment>> = OnceLock::new();
    INDEX.get_or_init(|| {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut found = Vec::new();
        let mut digits = [0u8; 6];
        for first in (0u32..=0xFF).filter(|octet| octet & 0b11 == 0) {
            for rest in 0u32..=0xFFFF {
                let prefix = (first << 16) | rest;
                for (index, digit) in digits.iter_mut().enumerate() {
                    *digit = HEX[((prefix >> (20 - 4 * index)) & 0xf) as usize];
                }
                // The buffer is six ASCII hex digits by construction.
                let text = std::str::from_utf8(&digits).unwrap_or("");
                if let Some(vendor) = mac::vendor(text) {
                    found.push(Assignment {
                        prefix: format!(
                            "{:02X}:{:02X}:{:02X}",
                            prefix >> 16,
                            (prefix >> 8) & 0xff,
                            prefix & 0xff
                        ),
                        vendor,
                    });
                }
            }
        }
        found
    })
}

/// How many octets of a MAC prefix the text holds, if that is what it is.
fn prefix_octets(text: &str) -> Option<usize> {
    let digits = text.chars().filter(|c| c.is_ascii_hexdigit()).count();
    let other = text
        .chars()
        .filter(|c| !c.is_ascii_hexdigit() && !matches!(c, ':' | '-' | '.' | ' ' | 'x' | 'X'))
        .count();
    (other == 0 && (6..=12).contains(&digits) && digits.is_multiple_of(2)).then_some(digits / 2)
}

fn registry_info() -> RegistryInfo {
    let (assignments, date) = mac::registry_info();
    RegistryInfo {
        assignments,
        date: date.to_string(),
    }
}

async fn oui(params: OuiParams) -> AppResult<OuiResult> {
    let query = params.query.trim().to_string();
    if query.is_empty() {
        return Err(AppError::invalid(
            "give a MAC address, an OUI prefix (`00:1b:21`) or part of a vendor's name",
        ));
    }
    let limit = params.limit.unwrap_or(OUI_LIMIT).clamp(1, 1000);
    if let Some(octets) = prefix_octets(&query) {
        // A prefix is an exact question with at most one answer.
        let assignments = match mac::vendor(&query) {
            Some(vendor) => vec![Assignment {
                prefix: mac::info(&query)
                    .map(|info| info.oui)
                    .unwrap_or_else(|_| query.to_uppercase()),
                vendor,
            }],
            None => Vec::new(),
        };
        return Ok(OuiResult {
            query,
            read_as: "prefix",
            total: assignments.len(),
            note: (octets == 3 && !assignments.is_empty()).then(|| {
                "A longer prefix finds the more specific MA-M, MA-S and IAB assignments inside this block.".to_string()
            }),
            assignments,
            registry: registry_info(),
        });
    }

    let wanted = query.clone();
    let matches = tokio::task::spawn_blocking(move || {
        vendor_index()
            .iter()
            .filter(|entry| util::matches_query(&entry.vendor, &wanted))
            .cloned()
            .collect::<Vec<Assignment>>()
    })
    .await
    .map_err(|err| AppError::internal(format!("the vendor search did not finish: {err}")))?;
    let total = matches.len();
    Ok(OuiResult {
        query,
        read_as: "vendor",
        assignments: matches.into_iter().take(limit).collect(),
        total,
        registry: registry_info(),
        note: Some(
            "Searching by name covers the 24-bit (MA-L) assignments. The smaller MA-M, MA-S and IAB blocks are in the registry too, but can only be looked up by prefix."
                .to_string(),
        ),
    })
}

fn oid(input: &str) -> AppResult<OidResult> {
    let text = input.trim();
    if text.is_empty() {
        return Err(AppError::invalid(
            "give an OID (`1.3.6.1.2.1.1.5.0`) or an object name (`sysName`, `IF-MIB::ifInOctets.3`)",
        ));
    }
    let resolved = names::resolve(text).or_else(|| text.parse::<Oid>().ok());
    Ok(match resolved {
        Some(oid) => OidResult {
            input: text.to_string(),
            name: names::describe(&oid),
            notification: names::trap_name(&oid).map(str::to_string),
            known: names::describe(&oid).is_some() || names::trap_name(&oid).is_some(),
            oid: Some(oid.to_string()),
        },
        None => OidResult {
            input: text.to_string(),
            oid: None,
            name: None,
            notification: None,
            known: false,
        },
    })
}

pub fn register(router: &mut Router) {
    router.add("tools.mac", |_: App, params: InputParams| async move {
        mac::info(&params.input).map_err(tool)
    });
    router.add("tools.oui", |_: App, params: OuiParams| async move {
        oui(params).await
    });
    router.add("tools.oid", |_: App, params: InputParams| async move {
        oid(&params.input)
    });
    router.add(
        "tools.interface",
        |_: App, params: InputParams| async move {
            let input = params.input.trim();
            if input.is_empty() {
                return Err(AppError::invalid(
                    "give an interface name, short (`Gi1/0/1`) or long (`GigabitEthernet1/0/1`)",
                ));
            }
            Ok(InterfaceResult {
                long: reference::interface_long_name(input),
                short: reference::interface_short_name(input),
            })
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prefix_is_told_from_a_vendor_name() {
        assert_eq!(prefix_octets("00:1b:21"), Some(3));
        assert_eq!(prefix_octets("001B21"), Some(3));
        assert_eq!(prefix_octets("0000.0c12.3456"), Some(6));
        assert_eq!(prefix_octets("00:1b"), None, "too short to be an OUI");
        assert_eq!(prefix_octets("Cisco"), None);
        // "Dell" and "ADDED" are hex-looking words; six digits of hex are
        // what a prefix needs, so "deadbe" is read as a prefix and "dell"
        // as a name. That is the right way round: a name search of a hex
        // word still works through the prefix path returning nothing found.
        assert_eq!(prefix_octets("dell"), None);
    }

    #[test]
    fn an_object_name_and_its_oid_resolve_to_each_other() {
        let by_name = oid("sysName.0").unwrap();
        assert_eq!(by_name.oid.as_deref(), Some("1.3.6.1.2.1.1.5.0"));
        assert_eq!(by_name.name.as_deref(), Some("sysName.0"));
        assert!(by_name.known);

        let by_number = oid("1.3.6.1.2.1.2.2.1.10.3").unwrap();
        assert_eq!(by_number.name.as_deref(), Some("ifInOctets.3"));

        // Nothing in the dictionary matches: that is an answer, not a guess.
        let unknown = oid("1.3.6.1.4.1.99999.1.2.3").unwrap();
        assert_eq!(unknown.name, None);
        assert!(!unknown.known);
        assert_eq!(unknown.oid.as_deref(), Some("1.3.6.1.4.1.99999.1.2.3"));

        let nonsense = oid("not an oid").unwrap();
        assert!(!nonsense.known && nonsense.oid.is_none());
        assert_eq!(oid("   ").unwrap_err().code, "invalid");
    }
}

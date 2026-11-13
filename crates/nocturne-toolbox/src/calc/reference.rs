//! The reference tables: the things an engineer looks up rather than works
//! out. Ports, DSCP, protocol and EtherType numbers, ICMP types,
//! administrative distances, Ethernet standards, syslog levels and
//! encapsulation overheads.
//!
//! Every row comes from `nocturne_tools::reference` (and, for the
//! encapsulations, `nocturne_tools::convert`), which took them from the RFCs
//! and the IEEE standards. Nothing is written down a second time here: a
//! wrong administrative distance in a reference tool is worse than a missing
//! one, and two copies of a table is how one of them becomes wrong.
//!
//! One table named in the specification has no engine behind it — cable
//! pinouts — so it is not offered rather than invented; `ethernet` is the
//! media table the engine does have. It is in the report.

use nocturne_app::{App, AppError, AppResult, Router, util};
use nocturne_tools::{convert, reference};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ReferenceParams {
    /// Which table: see `tools.reference.tables`.
    pub table: String,
    /// Free text, a number or a name; matched against the whole row.
    pub query: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct Table {
    pub table: &'static str,
    pub title: &'static str,
    /// The row fields, in the order a table should show them.
    pub columns: Vec<String>,
    pub rows: Vec<Value>,
    /// Rows matching the query, before `limit`.
    pub total: usize,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct TableInfo {
    pub table: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub rows: usize,
}

/// One reference table: its name, what it is, and how to produce its rows.
struct Descriptor {
    id: &'static str,
    title: &'static str,
    description: &'static str,
    aliases: &'static [&'static str],
    rows: fn() -> Vec<Value>,
}

/// Serialise a table of engine rows, skipping any row that cannot be (there
/// are none, but a panic in a reference tool would be absurd).
fn rows_of<T: Serialize>(rows: Vec<T>) -> Vec<Value> {
    rows.into_iter()
        .filter_map(|row| serde_json::to_value(row).ok())
        .collect()
}

const TABLES: &[Descriptor] = &[
    Descriptor {
        id: "ports",
        title: "Well-known and commonly met ports",
        description: "TCP and UDP ports a network engineer actually meets, with what listens on them.",
        aliases: &["port"],
        rows: || rows_of(reference::ports("")),
    },
    Descriptor {
        id: "dscp",
        title: "DSCP and IP precedence",
        description: "DSCP names and values with their ToS byte, precedence and the traffic each is meant for.",
        aliases: &["qos", "tos"],
        rows: || rows_of(reference::dscp_table()),
    },
    Descriptor {
        id: "protocols",
        title: "IP protocol numbers",
        description: "The protocol field of the IP header: 1 ICMP, 6 TCP, 47 GRE, 89 OSPF…",
        aliases: &["ip_protocols", "protocol"],
        rows: || rows_of(reference::ip_protocols()),
    },
    Descriptor {
        id: "ethertypes",
        title: "EtherTypes",
        description: "The type field of an Ethernet frame: 0x0800 IPv4, 0x0806 ARP, 0x8100 802.1Q…",
        aliases: &["ethertype"],
        rows: || rows_of(reference::ethertypes()),
    },
    Descriptor {
        id: "icmp_types",
        title: "ICMP types",
        description: "ICMPv4 message types and codes, for reading an ACL or a capture.",
        aliases: &["icmp"],
        rows: || rows_of(reference::icmp_types()),
    },
    Descriptor {
        id: "icmpv6_types",
        title: "ICMPv6 types",
        description: "ICMPv6 message types, including neighbour discovery.",
        aliases: &["icmpv6"],
        rows: || rows_of(reference::icmpv6_types()),
    },
    Descriptor {
        id: "admin_distance",
        title: "Administrative distance by vendor",
        description: "Which route wins, per platform: Cisco, Juniper, MikroTik, Huawei and Arista.",
        aliases: &["administrative_distances", "distance", "preference"],
        rows: || rows_of(reference::administrative_distances()),
    },
    Descriptor {
        id: "ethernet",
        title: "Ethernet standards and optics",
        description: "10BASE-T to 400GBASE: medium, speed, reach and the usual optics.",
        aliases: &["cables", "optics", "media"],
        rows: || rows_of(reference::ethernet_standards()),
    },
    Descriptor {
        id: "syslog_levels",
        title: "Syslog severities",
        description: "0 to 7 with the keyword each platform prints and what the level is for.",
        aliases: &["syslog", "severities"],
        rows: || rows_of(reference::syslog_levels()),
    },
    Descriptor {
        id: "encapsulations",
        title: "Encapsulation overheads",
        description: "What each tunnel or tag costs a packet; the input of tools.mtu.",
        aliases: &["overheads", "mtu"],
        rows: || rows_of(convert::encapsulations()),
    },
];

fn descriptor(name: &str) -> AppResult<&'static Descriptor> {
    let wanted = name.trim().to_ascii_lowercase();
    TABLES
        .iter()
        .find(|table| table.id == wanted || table.aliases.contains(&wanted.as_str()))
        .ok_or_else(|| {
            AppError::not_found(format!(
                "reference table `{name}` (there is {})",
                TABLES
                    .iter()
                    .map(|table| table.id)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })
}

/// The field names of a row, in the order the engine's struct declares them
/// (`serde_json`'s `preserve_order` feature keeps that order).
fn columns(row: Option<&Value>) -> Vec<String> {
    row.and_then(Value::as_object)
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default()
}

fn lookup(params: ReferenceParams) -> AppResult<Table> {
    let table = descriptor(&params.table)?;
    let all = (table.rows)();
    let columns = columns(all.first());
    let query = params
        .query
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty());
    let mut rows = match query {
        None => all,
        Some(query) => {
            // The ports and DSCP tables have their own search in the engine,
            // which knows that `ef` is 46 and that `0xb8` is a ToS byte.
            match table.id {
                "ports" => rows_of(reference::ports(query)),
                "dscp" => match reference::dscp(query) {
                    Some(entry) => rows_of(vec![entry]),
                    None => filtered(all, query),
                },
                _ => filtered(all, query),
            }
        }
    };
    let total = rows.len();
    let limit = params.limit.unwrap_or(500).clamp(1, 5000);
    rows.truncate(limit);
    Ok(Table {
        table: table.id,
        title: table.title,
        columns,
        rows,
        total,
        truncated: total > limit,
    })
}

/// Rows whose text contains every word of the query.
fn filtered(rows: Vec<Value>, query: &str) -> Vec<Value> {
    rows.into_iter()
        .filter(|row| util::matches_query(&row.to_string(), query))
        .collect()
}

pub fn register(router: &mut Router) {
    router.add(
        "tools.reference",
        |_: App, params: ReferenceParams| async move { lookup(params) },
    );
    router.add(
        "tools.reference.tables",
        |_: App, _: nocturne_app::NoParams| async move {
            Ok(TABLES
                .iter()
                .map(|table| TableInfo {
                    table: table.id,
                    title: table.title,
                    description: table.description,
                    rows: (table.rows)().len(),
                })
                .collect::<Vec<_>>())
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(name: &str, query: Option<&str>) -> Table {
        lookup(ReferenceParams {
            table: name.into(),
            query: query.map(str::to_string),
            limit: None,
        })
        .unwrap()
    }

    #[test]
    fn a_table_comes_back_with_its_columns_and_every_row() {
        let ports = table("ports", None);
        assert_eq!(ports.table, "ports");
        assert!(ports.total > 100, "{} ports", ports.total);
        assert_eq!(
            ports.columns,
            vec!["port", "protocol", "name", "description"]
        );
        assert!(!ports.truncated);

        // Aliases: an engineer types what they are thinking of.
        assert_eq!(table("qos", None).table, "dscp");
        assert_eq!(table("optics", None).table, "ethernet");
    }

    #[test]
    fn a_query_uses_the_engines_own_search_where_it_has_one() {
        // `ef` is DSCP 46 — the engine knows that; a text filter would not.
        let ef = table("dscp", Some("ef"));
        assert_eq!(ef.total, 1);
        assert_eq!(ef.rows[0]["dscp"], 46);
        assert_eq!(ef.rows[0]["tos"], 184);

        let ssh = table("ports", Some("22"));
        assert!(
            ssh.rows.iter().any(|row| row["port"] == 22),
            "{:?}",
            ssh.rows
        );

        // Everything else is a word filter over the row.
        let gre = table("protocols", Some("gre"));
        assert_eq!(gre.rows[0]["number"], 47);
        assert_eq!(table("protocols", Some("no such protocol")).total, 0);
    }

    #[test]
    fn an_unknown_table_is_not_found_and_says_what_there_is() {
        let refused = lookup(ReferenceParams {
            table: "pinouts".into(),
            ..ReferenceParams::default()
        })
        .unwrap_err();
        assert_eq!(refused.code, "not_found");
        assert!(refused.message.contains("ethernet"), "{}", refused.message);
    }
}

//! From a notification to the row that is stored for it.
//!
//! The row is built field by field from what was decoded. It is never the
//! decoded trap serialised: that structure carries the community, and a
//! body is returned by `logs.query`, published on the live tail and handed
//! to a forwarder.
//!
//! The message is what a person scanning a log wants to see and what a
//! rule matches: the notification's name and its bindings by name,
//! `linkDown ifIndex=3 (GigabitEthernet0/3) ifAdminStatus=up(1)
//! ifOperStatus=down(2)`. The body keeps every binding with its full name,
//! its OID and its value. Everything a device wrote goes through the
//! redactor on the way, and is cut to a length: a datagram is up to 64 KiB
//! of whatever its sender liked.

use std::borrow::Cow;
use std::collections::HashMap;

use nocturne_app::store::LogEntry;
use nocturne_snmp::{Value, VarBind, names};
use serde_json::{Map, Value as Json, json};

use super::Notification;
use crate::attribute::Index;
use crate::db::{self, Prepared};
use crate::guard;
use crate::rules::plain;
use crate::state::LogsState;

/// IF-MIB ifIndex: the column whose value is the interface's number.
const IF_INDEX: [u32; 10] = [1, 3, 6, 1, 2, 1, 2, 2, 1, 1];
/// IF-MIB ifEntry and ifXEntry: every column of them is indexed by ifIndex.
const IF_ENTRY: [u32; 9] = [1, 3, 6, 1, 2, 1, 2, 2, 1];
const IF_X_ENTRY: [u32; 10] = [1, 3, 6, 1, 2, 1, 31, 1, 1, 1];
/// ifAdminStatus and ifOperStatus, the two columns of ifEntry a link trap
/// carries as a number that means a word.
const IF_ADMIN_STATUS: u32 = 7;
const IF_OPER_STATUS: u32 = 8;

/// Bindings a message names. The rest are in the body, and counted.
pub const MAX_MESSAGE_VARBINDS: usize = 12;
/// The longest value a message shows, in characters.
pub const MAX_MESSAGE_VALUE: usize = 200;
/// Bindings a body keeps.
pub const MAX_BODY_VARBINDS: usize = 64;
/// The longest value a body keeps, in characters.
pub const MAX_BODY_VALUE: usize = 1_000;
/// What the bindings of one body may add up to, in bytes — OIDs and names
/// as well as values, since an OID can be over a kilobyte written out. An
/// entry goes onto the live tail, two hundred to an event.
pub const MAX_BODY_BYTES: usize = 16 * 1024;
/// The longest name a message shows for a binding, in characters: a name
/// nobody has is its OID, and that is as long as its sender liked.
pub const MAX_MESSAGE_NAME: usize = 160;
/// Bindings of a datagram that are looked at at all: more than a row can
/// show (the message names [`MAX_MESSAGE_VARBINDS`], the body keeps
/// [`MAX_BODY_VARBINDS`]). The rest are counted and dropped where the
/// datagram is decoded.
pub const MAX_READ_VARBINDS: usize = 128;
/// The longest interface name taken from the monitor's map.
pub const MAX_INTERFACE_NAME: usize = 200;

/// The ifIndex an ifIndex binding carries.
fn if_index_of(bind: &VarBind) -> Option<u32> {
    let [instance] = bind.oid.parts().strip_prefix(&IF_INDEX)? else {
        return None;
    };
    // The value is the index by definition; the instance says the same and
    // is what is left when an agent sent something else as the value.
    Some(
        bind.value
            .as_u64()
            .and_then(|index| u32::try_from(index).ok())
            .filter(|index| *index > 0)
            .unwrap_or(*instance),
    )
}

/// The instance of a binding that is a column of the interface tables.
fn if_table_instance(bind: &VarBind) -> Option<u32> {
    let parts = bind.oid.parts();
    match parts
        .strip_prefix(&IF_ENTRY)
        .or_else(|| parts.strip_prefix(&IF_X_ENTRY))?
    {
        [_column, instance] => Some(*instance),
        _ => None,
    }
}

/// The one interface a notification is about: the ifIndex it names, or the
/// index its interface-table bindings share. Two different ones are not
/// "an interface", and neither is none.
pub fn interface_index(varbinds: &[VarBind]) -> Option<u32> {
    fn agreed(mut found: impl Iterator<Item = u32>) -> Option<u32> {
        let first = found.next()?;
        found.all(|other| other == first).then_some(first)
    }
    agreed(varbinds.iter().filter_map(if_index_of))
        .or_else(|| agreed(varbinds.iter().filter_map(if_table_instance)))
}

/// Whether any binding could be named through an interface map at all — so
/// that the monitor is only asked for a notification that has one.
pub fn names_an_interface(varbinds: &[VarBind]) -> bool {
    varbinds
        .iter()
        .any(|bind| if_index_of(bind).is_some() || if_table_instance(bind).is_some())
}

/// `up(1)`: the word the standard gives a status number, with the number.
fn status_word(bind: &VarBind) -> Option<String> {
    let [column, _instance] = bind.oid.parts().strip_prefix(&IF_ENTRY)? else {
        return None;
    };
    let Value::Integer(number) = bind.value else {
        return None;
    };
    // RFC 2863: ifAdminStatus is up, down or testing; ifOperStatus adds
    // the rest.
    let word = match (*column, number) {
        (IF_ADMIN_STATUS | IF_OPER_STATUS, 1) => "up",
        (IF_ADMIN_STATUS | IF_OPER_STATUS, 2) => "down",
        (IF_ADMIN_STATUS | IF_OPER_STATUS, 3) => "testing",
        (IF_OPER_STATUS, 4) => "unknown",
        (IF_OPER_STATUS, 5) => "dormant",
        (IF_OPER_STATUS, 6) => "notPresent",
        (IF_OPER_STATUS, 7) => "lowerLayerDown",
        _ => return None,
    };
    Some(format!("{word}({number})"))
}

/// A value as text, before it is redacted and cut.
fn value_text(bind: &VarBind) -> String {
    if let Some(word) = status_word(bind) {
        return word;
    }
    match &bind.value {
        // An OID as a value is usually an object or a notification, and
        // reads better by its name.
        Value::Oid(oid) => names::describe(oid).unwrap_or_else(|| oid.to_string()),
        other => other.display(),
    }
}

/// The value as it may be kept.
///
/// The redactor recognises a secret by what stands in front of it, and for
/// a binding that is its name — `…Password.0` followed by a bare word — so
/// the two are shown to it together.
fn kept(state: &LogsState, name: &str, value: &str) -> String {
    let both = format!("{name}={value}");
    match guard::clean(&state.redactor, &both) {
        Cow::Borrowed(_) => value.to_string(),
        Cow::Owned(redacted) => match redacted
            .strip_prefix(name)
            .and_then(|rest| rest.strip_prefix('='))
        {
            Some(value) => value.to_string(),
            // The name was rewritten too. It is ours (out of the
            // dictionary, or a number), so that should not happen; the
            // value is then redacted on its own.
            None => guard::clean(&state.redactor, value).into_owned(),
        },
    }
}

/// The name a message uses for a binding: without the instance when the
/// instance says nothing (`.0` of a scalar, or the interface the whole
/// notification is about).
fn short_name<'a>(full: &'a str, bind: &VarBind, about: Option<u32>) -> &'a str {
    // An object's name has no dot in it; the first one starts the instance.
    let Some((base, instance)) = full.split_once('.') else {
        return full;
    };
    let is_scalar = instance == "0";
    let is_the_interface = about.is_some()
        && if_table_instance(bind) == about
        && instance.parse::<u32>().ok() == about;
    if is_scalar || is_the_interface {
        base
    } else {
        full
    }
}

/// What the notification is called: its name, or its OID when the
/// dictionary has no name for it.
fn headline(notification: &Notification) -> Option<String> {
    let oid = notification.trap_oid.as_ref()?;
    Some(
        names::trap_name(oid)
            .map(str::to_string)
            .unwrap_or_else(|| oid.to_string()),
    )
}

/// Build the row for one notification.
///
/// `interfaces` is the monitor's ifIndex → name map of the device, when it
/// has one. Without it an interface is shown by its number, which is what
/// is known.
pub fn prepare(
    state: &LogsState,
    index: &Index,
    notification: &Notification,
    interfaces: Option<&HashMap<u32, String>>,
) -> Prepared {
    let named = |number: u32| interfaces.and_then(|interfaces| interfaces.get(&number));
    let about = interface_index(&notification.varbinds);
    let headline = headline(notification);
    // The interface in the device's own collected spelling when it has one,
    // so that a trap and a syslog line about the same port carry the same
    // name; otherwise as the monitor's map spells it.
    let interface = about.and_then(named).map(|name| {
        index
            .interface_in(notification.device_id.as_deref(), name)
            .map(|(canonical, _)| canonical)
            .unwrap_or_else(|| name.clone())
    });

    let mut message = headline
        .clone()
        // A notification that does not say what it is (no snmpTrapOID).
        .unwrap_or_else(|| "(no snmpTrapOID)".to_string());
    let mut bindings = Vec::new();
    let mut spent = 0usize;
    // The body takes bindings in the order they were sent, and is closed by
    // the first one that does not fit. Going on to look for a smaller one
    // further down would name and redact every binding of a datagram built
    // to leave a few bytes free — thousands of them, for a row that shows
    // sixty-four.
    let mut body_open = true;
    for (position, bind) in notification.varbinds.iter().enumerate() {
        let in_message = position < MAX_MESSAGE_VARBINDS;
        body_open &= bindings.len() < MAX_BODY_VARBINDS;
        if !in_message && !body_open {
            // The rest are counted below as omitted.
            break;
        }
        let described = names::describe(&bind.oid);
        let dotted = bind.oid.to_string();
        let full = described.as_deref().unwrap_or(&dotted);
        let value = kept(state, full, &value_text(bind));
        let interface_name = if_index_of(bind).and_then(named);

        if in_message {
            let shown = plain(&value, MAX_MESSAGE_VALUE);
            message.push(' ');
            message.push_str(&plain(short_name(full, bind, about), MAX_MESSAGE_NAME));
            message.push('=');
            if shown.contains(char::is_whitespace) {
                // Quoted so that a reader, and a pattern, can tell where a
                // value with spaces in it ends.
                message.push('"');
                message.push_str(&shown);
                message.push('"');
            } else {
                message.push_str(&shown);
            }
            if let Some(name) = interface_name {
                message.push_str(" (");
                message.push_str(name);
                message.push(')');
            }
        }

        if !body_open {
            continue;
        }
        let cut = plain_body(&value);
        let cost = dotted.len() + described.as_ref().map_or(0, String::len) + cut.len();
        if spent + cost > MAX_BODY_BYTES {
            body_open = false;
        } else {
            spent += cost;
            let mut binding = Map::new();
            binding.insert("oid".into(), json!(dotted));
            // Null when the dictionary does not know the object: the OID is
            // then all there is, and a made-up name would be worse.
            binding.insert("name".into(), json!(described));
            binding.insert("value".into(), json!(cut));
            if let Some(name) = interface_name {
                binding.insert("interface".into(), json!(name));
            }
            bindings.push(Json::Object(binding));
        }
    }
    // Counted against what the datagram held, not against what was kept of it.
    let total = notification.varbinds_total.max(notification.varbinds.len());
    let unnamed = total.saturating_sub(MAX_MESSAGE_VARBINDS);
    if unnamed > 0 {
        message.push_str(&format!(" (+{unnamed} more)"));
    }
    let omitted = total - bindings.len();

    let mut body = Map::new();
    body.insert(
        "version".into(),
        json!(match notification.version {
            1 => "v1",
            2 => "v2c",
            _ => "v3",
        }),
    );
    body.insert(
        "trap".into(),
        json!(notification.trap_oid.as_ref().and_then(names::trap_name)),
    );
    body.insert(
        "oid".into(),
        json!(notification.trap_oid.as_ref().map(ToString::to_string)),
    );
    if let Some(ticks) = notification.uptime_ticks {
        // sysUpTime is in hundredths of a second.
        body.insert("uptime".into(), json!(f64::from(ticks) / 100.0));
    }
    if notification.inform {
        body.insert("inform".into(), json!(true));
    }
    if let Some(enterprise) = &notification.enterprise {
        body.insert("enterprise".into(), json!(enterprise.to_string()));
    }
    if let Some(generic) = notification.generic {
        body.insert("generic".into(), json!(generic));
    }
    if let Some(specific) = notification.specific {
        body.insert("specific".into(), json!(specific));
    }
    if let Some(agent) = notification.agent_addr {
        // What the agent says its own address is. The row is attributed by
        // the address the datagram came from, never by this.
        body.insert(
            "agent_addr".into(),
            json!(std::net::Ipv4Addr::from(agent).to_string()),
        );
    }
    body.insert("varbinds".into(), Json::Array(bindings));
    if omitted > 0 {
        body.insert("varbinds_omitted".into(), json!(omitted));
    }
    if let Some(number) = about {
        body.insert("if_index".into(), json!(number));
    }
    if let Some(interface) = &interface {
        body.insert("interface".into(), json!(interface));
    }

    Prepared {
        fp: db::fingerprint(&message),
        entry: LogEntry {
            id: 0,
            ts: notification.received_at,
            stream: db::TRAP_STREAM.into(),
            device_id: notification.device_id.clone(),
            // A notification has no syslog severity, and none is made up
            // for it: what it means is for a rule to say.
            severity: None,
            source: notification.source.clone(),
            message,
            body: Some(Json::Object(body)),
        },
        facility: None,
        app: None,
        // The notification's name is to a trap what the mnemonic is to a
        // syslog line, and the same column makes `logs.top` and a text
        // search work on both.
        mnemonic: headline,
        host: None,
        interface,
        reason: notification.reason.clone(),
    }
}

/// A value as a body keeps it: cut, but with its line breaks — the body is
/// where the whole value is looked at.
fn plain_body(value: &str) -> String {
    if value.chars().count() <= MAX_BODY_VALUE {
        return value.to_string();
    }
    let mut cut: String = value.chars().take(MAX_BODY_VALUE - 1).collect();
    cut.push('…');
    cut
}

/// The monitor's answer to `monitor.ifindex`, as a map. Names are text a
/// device wrote: cut, and without control characters.
pub fn interface_map(answer: &Json) -> HashMap<u32, String> {
    answer["interfaces"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|interface| {
            let number = u32::try_from(interface["index"].as_u64()?).ok()?;
            let name = plain(interface["name"].as_str()?.trim(), MAX_INTERFACE_NAME);
            (!name.is_empty()).then_some((number, name))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nocturne_snmp::Oid;

    fn oid(text: &str) -> Oid {
        text.parse().expect("an oid")
    }

    fn integer(name: &str, value: i64) -> VarBind {
        VarBind::new(oid(name), Value::Integer(value))
    }

    fn text(name: &str, value: &str) -> VarBind {
        VarBind::new(oid(name), Value::OctetString(value.as_bytes().to_vec()))
    }

    fn notification(trap: &str, varbinds: Vec<VarBind>) -> Notification {
        Notification {
            version: 2,
            source: "192.0.2.7".into(),
            device_id: None,
            reason: "no device has this address".into(),
            received_at: 1_763_000_000_000,
            uptime_ticks: Some(12_345),
            trap_oid: Some(oid(trap)),
            enterprise: None,
            agent_addr: None,
            generic: None,
            specific: None,
            varbinds_total: varbinds.len(),
            varbinds,
            inform: false,
        }
    }

    fn link_down() -> Notification {
        notification(
            "1.3.6.1.6.3.1.1.5.3",
            vec![
                integer("1.3.6.1.2.1.2.2.1.1.3", 3),
                integer("1.3.6.1.2.1.2.2.1.7.3", 1),
                integer("1.3.6.1.2.1.2.2.1.8.3", 2),
            ],
        )
    }

    fn row(notification: &Notification, interfaces: Option<&HashMap<u32, String>>) -> Prepared {
        prepare(
            &LogsState::default(),
            &Index::default(),
            notification,
            interfaces,
        )
    }

    #[test]
    fn a_link_trap_reads_as_its_name_and_its_bindings_by_name() {
        let prepared = row(&link_down(), None);
        assert_eq!(
            prepared.entry.message,
            "linkDown ifIndex=3 ifAdminStatus=up(1) ifOperStatus=down(2)"
        );
        assert_eq!(prepared.entry.stream, "trap");
        assert_eq!(
            prepared.entry.severity, None,
            "a trap has no syslog severity"
        );
        assert_eq!(prepared.mnemonic.as_deref(), Some("linkDown"));
        assert_eq!(
            prepared.interface, None,
            "nothing knows what ifIndex 3 is called"
        );
        let body = prepared.entry.body.unwrap();
        assert_eq!(body["version"], json!("v2c"));
        assert_eq!(body["trap"], json!("linkDown"));
        assert_eq!(body["oid"], json!("1.3.6.1.6.3.1.1.5.3"));
        assert_eq!(body["uptime"], json!(123.45));
        assert_eq!(body["if_index"], json!(3));
        assert_eq!(body.get("interface"), None);
        assert_eq!(
            body["varbinds"],
            json!([
                {"oid": "1.3.6.1.2.1.2.2.1.1.3", "name": "ifIndex.3", "value": "3"},
                {"oid": "1.3.6.1.2.1.2.2.1.7.3", "name": "ifAdminStatus.3", "value": "up(1)"},
                {"oid": "1.3.6.1.2.1.2.2.1.8.3", "name": "ifOperStatus.3", "value": "down(2)"},
            ])
        );
    }

    #[test]
    fn an_interface_is_named_when_the_monitor_knows_it_and_numbered_when_it_does_not() {
        let interfaces = HashMap::from([(3, "GigabitEthernet0/3".to_string())]);
        let named = row(&link_down(), Some(&interfaces));
        assert_eq!(
            named.entry.message,
            "linkDown ifIndex=3 (GigabitEthernet0/3) ifAdminStatus=up(1) ifOperStatus=down(2)"
        );
        assert_eq!(named.interface.as_deref(), Some("GigabitEthernet0/3"));
        let body = named.entry.body.unwrap();
        assert_eq!(body["interface"], json!("GigabitEthernet0/3"));
        assert_eq!(
            body["varbinds"][0]["interface"],
            json!("GigabitEthernet0/3")
        );

        // A map that does not have this index names nothing: the number
        // is what is known, and it is not dressed up.
        let other = HashMap::from([(4, "GigabitEthernet0/4".to_string())]);
        let numbered = row(&link_down(), Some(&other));
        assert_eq!(
            numbered.entry.message,
            "linkDown ifIndex=3 ifAdminStatus=up(1) ifOperStatus=down(2)"
        );
        assert_eq!(numbered.interface, None);
    }

    #[test]
    fn a_trap_nobody_has_a_name_for_is_shown_by_its_numbers() {
        let prepared = row(
            &notification(
                "1.3.6.1.4.1.99999.0.7",
                vec![
                    text("1.3.6.1.4.1.99999.1.1.0", "fan 2"),
                    text("1.3.6.1.2.1.1.5.0", "edge-sw-7"),
                ],
            ),
            None,
        );
        assert_eq!(
            prepared.entry.message,
            "1.3.6.1.4.1.99999.0.7 1.3.6.1.4.1.99999.1.1.0=\"fan 2\" sysName=edge-sw-7"
        );
        assert_eq!(prepared.mnemonic.as_deref(), Some("1.3.6.1.4.1.99999.0.7"));
        let body = prepared.entry.body.unwrap();
        assert_eq!(body["trap"], Json::Null, "no name is invented for it");
        assert_eq!(body["varbinds"][0]["name"], Json::Null);
        assert_eq!(body["varbinds"][1]["name"], json!("sysName.0"));
    }

    #[test]
    fn an_instance_that_says_something_stays_in_the_name() {
        // Two interfaces in one notification: neither is "the" interface,
        // and each binding keeps the index that tells them apart.
        let prepared = row(
            &notification(
                "1.3.6.1.4.1.99999.0.9",
                vec![
                    integer("1.3.6.1.2.1.2.2.1.8.3", 2),
                    integer("1.3.6.1.2.1.2.2.1.8.4", 1),
                ],
            ),
            None,
        );
        assert_eq!(
            prepared.entry.message,
            "1.3.6.1.4.1.99999.0.9 ifOperStatus.3=down(2) ifOperStatus.4=up(1)"
        );
        assert_eq!(prepared.entry.body.unwrap().get("if_index"), None);
    }

    #[test]
    fn a_secret_in_a_binding_is_not_stored() {
        let prepared = row(
            &notification(
                "1.3.6.1.4.1.99999.0.7",
                vec![
                    text(
                        "1.3.6.1.4.1.99999.1.1.0",
                        "snmp-server community Str0ngR0 RO",
                    ),
                    // Nothing in the value says what it is; its name does.
                    text("1.3.6.1.4.1.99999.1.2.0", "hunter2hunter2"),
                ],
            ),
            None,
        );
        let stored = format!(
            "{} {}",
            prepared.entry.message,
            prepared.entry.body.as_ref().unwrap()
        );
        assert!(!stored.contains("Str0ngR0"), "{stored}");
        assert!(stored.contains("<secret:"), "{stored}");
        assert!(
            stored.contains("hunter2hunter2"),
            "a bare word under a number is not known to be a secret: {stored}"
        );

        let state = LogsState::default();
        assert_eq!(
            kept(&state, "password", "hunter2hunter2"),
            kept(&state, "password", "hunter2hunter2"),
        );
        assert!(
            !kept(&state, "password", "hunter2hunter2").contains("hunter2"),
            "a value is redacted for what its name says it is"
        );
    }

    #[test]
    fn what_a_sender_stuffed_into_a_datagram_is_cut_and_counted() {
        let long = "x".repeat(5_000);
        let varbinds: Vec<VarBind> = (0..200u32)
            .map(|index| text(&format!("1.3.6.1.4.1.99999.1.{index}.0"), &long))
            .collect();
        let prepared = row(&notification("1.3.6.1.4.1.99999.0.7", varbinds), None);
        assert!(
            prepared.entry.message.chars().count()
                < MAX_MESSAGE_VARBINDS * (MAX_MESSAGE_VALUE + 40) + 100,
            "{}",
            prepared.entry.message.len()
        );
        assert!(prepared.entry.message.ends_with("(+188 more)"));
        let body = prepared.entry.body.unwrap();
        let kept = body["varbinds"].as_array().unwrap().len();
        assert!(kept <= MAX_BODY_VARBINDS, "{kept}");
        assert_eq!(body["varbinds_omitted"], json!(200 - kept));
        let bytes: usize = body["varbinds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|bind| bind["value"].as_str().unwrap().len() + bind["oid"].as_str().unwrap().len())
            .sum();
        assert!(bytes <= MAX_BODY_BYTES, "{bytes}");
        assert!(kept >= 10, "what fits is kept: {kept}");
        assert!(
            body["varbinds"][0]["value"]
                .as_str()
                .unwrap()
                .ends_with('…'),
            "a cut value says that it was cut"
        );
    }

    #[test]
    fn a_datagram_built_to_leave_the_body_a_few_bytes_short_is_not_searched_to_its_end() {
        // Sixteen values that leave less room than any binding needs, and
        // nine thousand more behind them. Looking through those for one
        // that might still fit named and redacted every one of them: eight
        // seconds for one datagram, in a build like this one.
        let mut varbinds: Vec<VarBind> = (0..15u32)
            .map(|index| text(&format!("1.3.6.1.4.1.99999.2.{index}"), &"v".repeat(1_060)))
            .collect();
        varbinds.extend(
            (0..9_000u32)
                .map(|index| text(&format!("1.3.6.1.4.1.99999.3.{index}"), &"w".repeat(900))),
        );
        let hostile = notification("1.3.6.1.4.1.99999.0.7", varbinds);
        let started = std::time::Instant::now();
        let prepared = row(&hostile, None);
        let took = started.elapsed();
        assert!(
            took < std::time::Duration::from_secs(1),
            "one datagram took {took:?} to turn into a row"
        );
        assert!(prepared.entry.message.ends_with("(+9003 more)"));
        let body = prepared.entry.body.unwrap();
        let kept = body["varbinds"].as_array().unwrap();
        // In the order they were sent, up to the first that did not fit.
        assert_eq!(kept.len(), 16);
        assert_eq!(kept[15]["oid"], json!("1.3.6.1.4.1.99999.3.0"));
        assert_eq!(body["varbinds_omitted"], json!(9_015 - 16));
    }

    #[test]
    fn what_was_dropped_where_the_datagram_was_decoded_is_still_counted() {
        // The receiver keeps the first bindings of a datagram and says how
        // many there were.
        let mut trap = link_down();
        trap.varbinds_total = 6_003;
        let prepared = row(&trap, None);
        assert_eq!(
            prepared.entry.message,
            "linkDown ifIndex=3 ifAdminStatus=up(1) ifOperStatus=down(2) (+5991 more)"
        );
        let body = prepared.entry.body.unwrap();
        assert_eq!(body["varbinds"].as_array().unwrap().len(), 3);
        assert_eq!(body["varbinds_omitted"], json!(6_000));
    }

    #[test]
    fn an_oid_as_long_as_its_sender_liked_is_cut_in_the_message_and_counted_in_the_body() {
        // 120 sub-identifiers of ten digits: over a kilobyte written out.
        let long: Vec<u32> = [1, 3, 6, 1, 4, 1]
            .into_iter()
            .chain(std::iter::repeat_n(4_000_000_000, 120))
            .collect();
        let varbinds: Vec<VarBind> = (0..60u32)
            .map(|index| {
                let mut parts = long.clone();
                parts.push(index);
                VarBind::new(Oid::new(parts), Value::Integer(1))
            })
            .collect();
        let prepared = row(&notification("1.3.6.1.4.1.99999.0.7", varbinds), None);
        assert!(
            prepared.entry.message.chars().count()
                < MAX_MESSAGE_VARBINDS * (MAX_MESSAGE_NAME + 10) + 100,
            "{}",
            prepared.entry.message.len()
        );
        let body = prepared.entry.body.unwrap();
        let bytes: usize = body["varbinds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|bind| bind["oid"].as_str().unwrap().len())
            .sum();
        assert!(bytes <= MAX_BODY_BYTES, "{bytes}");
        let kept = body["varbinds"].as_array().unwrap().len();
        assert!((1..60).contains(&kept), "{kept}");
        assert_eq!(body["varbinds_omitted"], json!(60 - kept));
    }

    #[test]
    fn a_control_character_in_a_value_does_not_reach_the_message() {
        let prepared = row(
            &notification(
                "1.3.6.1.4.1.99999.0.7",
                vec![text("1.3.6.1.2.1.1.1.0", "line one\r\nline two\tend")],
            ),
            None,
        );
        assert_eq!(
            prepared.entry.message,
            "1.3.6.1.4.1.99999.0.7 sysDescr=\"line one  line two end\""
        );
    }

    #[test]
    fn a_v1_trap_keeps_what_only_v1_says() {
        let mut trap = link_down();
        trap.version = 1;
        trap.enterprise = Some(oid("1.3.6.1.4.1.8072.3.2.10"));
        trap.generic = Some(2);
        trap.specific = Some(0);
        trap.agent_addr = Some([192, 0, 2, 1]);
        let body = row(&trap, None).entry.body.unwrap();
        assert_eq!(body["version"], json!("v1"));
        assert_eq!(body["enterprise"], json!("1.3.6.1.4.1.8072.3.2.10"));
        assert_eq!(
            (&body["generic"], &body["specific"]),
            (&json!(2), &json!(0))
        );
        assert_eq!(body["agent_addr"], json!("192.0.2.1"));
    }

    #[test]
    fn the_monitors_map_is_read_and_its_names_are_only_text() {
        let map = interface_map(&json!({"at": 1, "interfaces": [
            {"index": 3, "name": "GigabitEthernet0/3"},
            {"index": 4, "name": "  "},
            {"index": 5, "name": "Gi0/5\u{1b}[31m"},
            {"index": "six", "name": "x"},
        ]}));
        assert_eq!(map.get(&3).map(String::as_str), Some("GigabitEthernet0/3"));
        assert_eq!(map.get(&4), None, "a blank name names nothing");
        assert_eq!(map.get(&5).map(String::as_str), Some("Gi0/5 [31m"));
        assert_eq!(map.len(), 2);
        assert!(interface_map(&json!({"interfaces": null})).is_empty());
    }
}

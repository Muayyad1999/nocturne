//! What the devices said about themselves.
//!
//! Two checks have no other source. A port that the switch disabled itself is
//! only visible in its log (the collected interface model has no
//! "err-disabled" state), and a port that flapped between two collections
//! left no trace in either of them. Both are read from the stored log stream,
//! which the logs feature fills from syslog — so both say "not evaluated" for
//! a device that does not send its log anywhere, instead of calling it healthy.
//!
//! One query per scan, not one per device: the log table is the largest thing
//! in the database.

use std::collections::{HashMap, HashSet};

use nocturne_app::AppResult;
use nocturne_app::store::{LogQuery, Store};
use nocturne_drivers::ifname;
use regex::Regex;
use std::sync::LazyLock;

/// `%LINK-3-UPDOWN: Interface GigabitEthernet0/5, changed state to down`,
/// `%LINEPROTO-5-UPDOWN: Line protocol on Interface Gi0/5, changed state to up`.
static IOS_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)Interface\s+([A-Za-z][\w/.:\-]*)\s*,\s*changed state to\s+(up|down)")
        .expect("link pattern")
});
/// RouterOS: `interface,info ether3 link down`.
static ROUTEROS_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b([\w/.\-]+)\s+link\s+(up|down)\b").expect("link pattern"));
/// `%PM-4-ERR_DISABLE: bpduguard error detected on Gi0/5, putting Gi0/5 in err-disable state`.
static ERR_DISABLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)putting\s+([A-Za-z][\w/.:\-]*)\s+in\s+err-?disable")
        .expect("err-disable pattern")
});
/// The cause the platform printed: `bpduguard error detected`, `Loop-back detected`.
static ERR_CAUSE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)%(?:[A-Z_0-9]+)-\d-([A-Z_0-9]+)\s*:\s*(.*?)(?:,\s*putting|$)")
        .expect("cause pattern")
});

/// A link going up or down, as a device reported it.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkEvent {
    pub device_id: String,
    /// Canonical long form.
    pub interface: String,
    pub up: bool,
    pub at: i64,
}

/// A port the switch disabled itself.
#[derive(Clone, Debug, PartialEq)]
pub struct ErrDisable {
    pub device_id: String,
    pub interface: String,
    /// What the platform said, e.g. `bpduguard error detected`.
    pub cause: String,
    pub message: String,
    pub at: i64,
}

fn query(since: i64, text: &str, limit: u32) -> LogQuery {
    LogQuery {
        stream: Some("syslog".into()),
        since: Some(since),
        text: Some(text.into()),
        limit: Some(limit),
        ..LogQuery::default()
    }
}

/// Link up/down events since `since`, grouped by device.
pub fn link_events(store: &Store, since: i64) -> AppResult<HashMap<String, Vec<LinkEvent>>> {
    let mut events: HashMap<String, Vec<LinkEvent>> = HashMap::new();
    // Two narrow text filters rather than a scan: IOS-style and RouterOS-style.
    for (text, pattern) in [
        ("changed state to", &*IOS_LINK),
        ("link down", &*ROUTEROS_LINK),
        ("link up", &*ROUTEROS_LINK),
    ] {
        for entry in store.log_query(&query(since, text, 5_000))? {
            let Some(device_id) = entry.device_id.clone() else {
                continue;
            };
            let Some(captures) = pattern.captures(&entry.message) else {
                continue;
            };
            let interface = ifname::long(captures.get(1).map_or("", |name| name.as_str()));
            let up = captures
                .get(2)
                .is_some_and(|state| state.as_str().eq_ignore_ascii_case("up"));
            let event = LinkEvent {
                device_id: device_id.clone(),
                interface,
                up,
                at: entry.ts,
            };
            let list = events.entry(device_id).or_default();
            if !list.contains(&event) {
                list.push(event);
            }
        }
    }
    Ok(events)
}

/// Err-disable events since `since`, newest first per device.
pub fn err_disable_events(
    store: &Store,
    since: i64,
) -> AppResult<HashMap<String, Vec<ErrDisable>>> {
    let mut events: HashMap<String, Vec<ErrDisable>> = HashMap::new();
    for entry in store.log_query(&query(since, "err-disable", 2_000))? {
        let Some(device_id) = entry.device_id.clone() else {
            continue;
        };
        let Some(captures) = ERR_DISABLE.captures(&entry.message) else {
            continue;
        };
        let cause = ERR_CAUSE
            .captures(&entry.message)
            .and_then(|captures| captures.get(2))
            .map(|cause| cause.as_str().trim().to_string())
            .filter(|cause| !cause.is_empty())
            .unwrap_or_else(|| "the switch disabled the port".to_string());
        events
            .entry(device_id.clone())
            .or_default()
            .push(ErrDisable {
                device_id,
                interface: ifname::long(captures.get(1).map_or("", |name| name.as_str())),
                cause,
                message: entry.message.clone(),
                at: entry.ts,
            });
    }
    Ok(events)
}

/// Devices that logged anything at all in the window. Without this, "no
/// err-disable entries" and "this device does not send its log to us" would
/// be the same answer.
pub fn devices_with_logs(
    store: &Store,
    device_ids: &[&str],
    since: i64,
) -> AppResult<HashSet<String>> {
    let mut present = HashSet::new();
    // Where nothing was logged at all — no syslog receiver, which is the
    // common case — one query answers for every device.
    let any = store.log_query(&LogQuery {
        since: Some(since),
        limit: Some(1),
        ..LogQuery::default()
    })?;
    if any.is_empty() {
        return Ok(present);
    }
    for device_id in device_ids {
        let any = store.log_query(&LogQuery {
            device_id: Some((*device_id).to_string()),
            since: Some(since),
            limit: Some(1),
            ..LogQuery::default()
        })?;
        if !any.is_empty() {
            present.insert((*device_id).to_string());
        }
    }
    Ok(present)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nocturne_app::store::LogEntry;

    fn append(store: &Store, device_id: &str, at: i64, message: &str) {
        store
            .log_append(&LogEntry {
                id: 0,
                ts: at,
                stream: "syslog".into(),
                device_id: Some(device_id.into()),
                severity: Some(5),
                source: "10.0.0.2".into(),
                message: message.into(),
                body: None,
            })
            .expect("append");
    }

    #[test]
    fn reads_link_events_from_both_vendors_log_shapes() {
        let store = Store::in_memory().unwrap();
        append(
            &store,
            "a",
            1_000,
            "%LINK-3-UPDOWN: Interface GigabitEthernet0/5, changed state to down",
        );
        append(
            &store,
            "a",
            2_000,
            "%LINEPROTO-5-UPDOWN: Line protocol on Interface Gi0/5, changed state to up",
        );
        append(&store, "b", 3_000, "interface,info ether3 link down");
        append(
            &store,
            "b",
            4_000,
            "%SYS-5-CONFIG_I: Configured from console by admin",
        );
        let events = link_events(&store, 0).unwrap();
        let first = &events["a"];
        assert_eq!(first.len(), 2);
        assert!(
            first
                .iter()
                .all(|event| event.interface == "GigabitEthernet0/5")
        );
        assert_eq!(events["b"].len(), 1);
        assert_eq!(events["b"][0].interface, "ether3");
        assert!(!events["b"][0].up);
        // The window is honoured.
        assert!(link_events(&store, 5_000).unwrap().is_empty());
    }

    #[test]
    fn reads_err_disable_with_the_cause_the_platform_printed() {
        let store = Store::in_memory().unwrap();
        append(
            &store,
            "a",
            1_000,
            "%PM-4-ERR_DISABLE: bpduguard error detected on Gi0/5, putting Gi0/5 in err-disable state",
        );
        append(
            &store,
            "a",
            2_000,
            "%ETHCNTR-3-LOOP_BACK_DETECTED: Loop-back detected on Gi0/7.  Putting Gi0/7 in err-disable state.",
        );
        append(
            &store,
            "a",
            3_000,
            "%LINK-3-UPDOWN: Interface Gi0/9, changed state to down",
        );
        let events = err_disable_events(&store, 0).unwrap();
        let mut found: Vec<(String, String)> = events["a"]
            .iter()
            .map(|event| (event.interface.clone(), event.cause.clone()))
            .collect();
        found.sort();
        assert_eq!(found[0].0, "GigabitEthernet0/5");
        assert_eq!(found[0].1, "bpduguard error detected on Gi0/5");
        assert_eq!(found[1].0, "GigabitEthernet0/7");
        assert!(found[1].1.contains("Loop-back detected"));

        assert_eq!(devices_with_logs(&store, &["a", "b"], 0).unwrap().len(), 1);
    }
}

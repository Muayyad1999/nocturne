//! Whether a notification is kept, decided from the one thing in it that is
//! a credential — and the last place that credential exists.
//!
//! An SNMPv1 or v2c notification carries a community. It is the only thing
//! that tells a trap a switch sent from one anybody on the network wrote,
//! and it is the same string that reads the switch's whole MIB: a secret.
//! So the community is compared here, against the one the vault holds for
//! the device the datagram's source address belongs to, and then dropped.
//! Nothing after this module has it: not the queue, not the row, not an
//! error, not a log line. A rejection is remembered as an address, a device
//! and a sentence, never as what was tried.
//!
//! There is deliberately no list of accepted communities in the settings.
//! A settings document is returned by `settings.get`.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;

use nocturne_app::model::Device;
use nocturne_app::{App, credentials, inventory};
use nocturne_snmp::{Value, VarBind};

use crate::model::Rejection;

/// How long what the vault said about a device is used before it is asked
/// again. The cache is also emptied when a credential, a device or the
/// vault changes, so this only bounds what happens behind the events.
const KNOWN_MAX_AGE_MS: i64 = 30_000;
/// Addresses whose rejections are remembered for `logs.status`. A flood of
/// forged sources must not grow this, so beyond it the one heard from
/// longest ago makes room.
pub const MAX_REJECTIONS: usize = 64;
/// How many of them one status carries.
pub const SHOWN_REJECTIONS: usize = 20;
/// What stands where a community was. It names nothing and can be turned
/// back into nothing — unlike the redactor's `<secret:n>`, which keeps the
/// value it replaced so that a model's answer can be given it back.
pub const WITHHELD: &str = "<community>";
/// A community shorter than this is only withheld from a value that is
/// exactly it: two letters occur inside too much ordinary text.
const MIN_COMMUNITY_INSIDE: usize = 4;

/// snmpTrapCommunity (RFC 3584): what a proxy adds to a trap it forwards —
/// the community the original was sent with.
const SNMP_TRAP_COMMUNITY: [u32; 9] = [1, 3, 6, 1, 6, 3, 18, 1, 4];
/// snmpCommunityName (RFC 3584), a column of snmpCommunityTable.
const SNMP_COMMUNITY_NAME: [u32; 11] = [1, 3, 6, 1, 6, 3, 18, 1, 1, 1, 2];

/// What a device is known to send with.
pub enum Known {
    /// SNMPv1/v2c: the community, still inside the type the vault handed
    /// it over in, which wipes it when it is dropped and prints only
    /// whether it is set.
    Community(credentials::Resolved),
    /// SNMPv3 without authentication: the user name is all a notification
    /// carries that can be compared.
    User(String),
    /// Nothing can be compared, and why.
    Nothing(String),
}

/// What the vault said about each device, for a short while.
#[derive(Default)]
pub struct KnownCache {
    by_device: HashMap<String, (i64, Arc<Known>)>,
}

impl KnownCache {
    pub fn clear(&mut self) {
        self.by_device.clear();
    }
}

/// Read what `device` is known to send with from its SNMP credential — the
/// one the monitor polls it with.
pub fn lookup(app: &App, device: &Device) -> Known {
    let Some(credential_id) = device
        .monitor
        .snmp_credential_id
        .as_ref()
        .or(device.credential_id.as_ref())
    else {
        return Known::Nothing(format!(
            "`{}` has no credential, so no community is stored for it",
            device.name
        ));
    };
    let resolved = match credentials::resolve(app, credential_id) {
        Ok(resolved) => resolved,
        // The code and nothing else: an error's message is written by the
        // code that failed, and this one failed holding secrets.
        Err(err) => {
            return Known::Nothing(format!(
                "the credential of `{}` could not be read from the vault ({}); \
                 a locked vault checks no community",
                device.name, err.code
            ));
        }
    };
    match resolved.snmp.version.as_str() {
        "v1" | "v2c"
            if resolved
                .snmp_community
                .as_ref()
                .is_some_and(|c| !c.is_empty()) =>
        {
            // Only the community is kept: the rest of what was decrypted
            // (passwords, a private key) is wiped here, with `resolved`.
            Known::Community(credentials::Resolved {
                snmp_community: resolved.snmp_community.clone(),
                ..credentials::Resolved::default()
            })
        }
        "v3" if resolved.snmp.auth_protocol.is_empty() && !resolved.snmp.user.is_empty() => {
            Known::User(resolved.snmp.user.clone())
        }
        "v3" => Known::Nothing(format!(
            "the SNMP credential of `{}` is SNMPv3 with authentication: it holds no community, \
             and a notification that is not authenticated cannot be from it",
            device.name
        )),
        _ => Known::Nothing(format!(
            "the SNMP credential of `{}` holds no SNMPv1/v2c community",
            device.name
        )),
    }
}

fn known(
    app: &App,
    cache: &parking_lot::Mutex<KnownCache>,
    device_id: &str,
    now: i64,
) -> Arc<Known> {
    if let Some((at, known)) = cache.lock().by_device.get(device_id)
        && now - at < KNOWN_MAX_AGE_MS
    {
        return known.clone();
    }
    // Read outside the lock: the vault decrypts, and the status reads this
    // cache's neighbours.
    let known = Arc::new(match inventory::get(app, device_id) {
        Ok(device) => lookup(app, &device),
        Err(_) => Known::Nothing("the device is no longer in the inventory".to_string()),
    });
    cache
        .lock()
        .by_device
        .insert(device_id.to_string(), (now, known.clone()));
    known
}

/// Whether two byte strings are the same, looking at every byte whatever
/// the answer. An inform is acknowledged when its community is right, so
/// the time this takes is the one thing about the comparison a sender can
/// observe.
fn same(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |differs, (a, b)| differs | (a ^ b))
            == 0
}

/// Whether a notification carries what the device it claims to be from is
/// known to send with. `Err` is the reason it does not, in a sentence that
/// never contains what was sent.
///
/// `version` is the notification's (1, 2 for v2c, 3) and `presented` its
/// community — for SNMPv3 without authentication, its user name.
pub fn check(
    app: &App,
    cache: &parking_lot::Mutex<KnownCache>,
    device: Option<(&str, &str)>,
    unattributed: &str,
    version: u8,
    presented: &str,
    now: i64,
) -> Result<(), String> {
    let Some((device_id, device_name)) = device else {
        return Err(unattributed.to_string());
    };
    match (&*known(app, cache, device_id, now), version) {
        (Known::Community(held), 1 | 2) => {
            let community = held
                .snmp_community
                .as_ref()
                .map(|community| community.as_bytes())
                .unwrap_or_default();
            if same(community, presented.as_bytes()) {
                Ok(())
            } else {
                Err(format!(
                    "the community is not the one stored for `{device_name}`"
                ))
            }
        }
        (Known::Community(_), _) => Err(format!(
            "`{device_name}` is stored with an SNMPv1/v2c community and this was SNMPv3 without authentication"
        )),
        (Known::User(user), 3) => {
            if same(user.as_bytes(), presented.as_bytes()) {
                Ok(())
            } else {
                Err(format!(
                    "the SNMPv3 user is not the one stored for `{device_name}`"
                ))
            }
        }
        (Known::User(_), _) => Err(format!(
            "`{device_name}` is stored with an SNMPv3 user and this was SNMPv1/v2c"
        )),
        (Known::Nothing(why), _) => Err(why.clone()),
    }
}

fn replace_all(haystack: &[u8], needle: &[u8], with: &[u8]) -> Option<Vec<u8>> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    let mut out = Vec::new();
    let mut found = false;
    let mut at = 0;
    while at < haystack.len() {
        if haystack[at..].starts_with(needle) {
            out.extend_from_slice(with);
            at += needle.len();
            found = true;
        } else {
            out.push(haystack[at]);
            at += 1;
        }
    }
    found.then_some(out)
}

/// Take the community out of the bindings.
///
/// It is not only in the message header. A proxy that forwards a trap adds
/// `snmpTrapCommunity.0` with the community of the original (RFC 3584), a
/// device can be asked to report its own community table, and nothing
/// stops an agent putting it in a binding of its own. So the two objects
/// that are a community by definition are emptied whatever they hold, and
/// the datagram's own community is cut out of every string it appears in.
pub fn scrub(varbinds: &mut [VarBind], community: &[u8]) {
    for bind in varbinds {
        let parts = bind.oid.parts();
        let is_community =
            parts.starts_with(&SNMP_TRAP_COMMUNITY) || parts.starts_with(&SNMP_COMMUNITY_NAME);
        let (Value::OctetString(bytes) | Value::Opaque(bytes)) = &mut bind.value else {
            continue;
        };
        if is_community || (!community.is_empty() && bytes.as_slice() == community) {
            *bytes = WITHHELD.as_bytes().to_vec();
        } else if community.len() >= MIN_COMMUNITY_INSIDE
            && let Some(without) = replace_all(bytes, community, WITHHELD.as_bytes())
        {
            *bytes = without;
        }
    }
}

/// The sources whose notifications were turned away lately, so that a
/// rising `rejected` count comes with something to act on. In memory only,
/// and bounded: this is fed by whoever can reach the port.
#[derive(Default)]
pub struct Rejections {
    by_address: HashMap<IpAddr, Rejection>,
}

impl Rejections {
    pub fn record(&mut self, address: IpAddr, device_id: Option<&str>, reason: String, at: i64) {
        if let Some(seen) = self.by_address.get_mut(&address) {
            seen.count += 1;
            seen.last_at = at;
            seen.device_id = device_id.map(str::to_string);
            seen.reason = reason;
            return;
        }
        if self.by_address.len() >= MAX_REJECTIONS
            && let Some(oldest) = self
                .by_address
                .iter()
                .min_by_key(|(_, seen)| seen.last_at)
                .map(|(address, _)| *address)
        {
            self.by_address.remove(&oldest);
        }
        self.by_address.insert(
            address,
            Rejection {
                address: address.to_string(),
                device_id: device_id.map(str::to_string),
                reason,
                count: 1,
                last_at: at,
            },
        );
    }

    /// The most recent, newest first.
    pub fn recent(&self) -> Vec<Rejection> {
        let mut recent: Vec<Rejection> = self.by_address.values().cloned().collect();
        recent.sort_by(|left, right| {
            right
                .last_at
                .cmp(&left.last_at)
                .then_with(|| left.address.cmp(&right.address))
        });
        recent.truncate(SHOWN_REJECTIONS);
        recent
    }

    pub fn len(&self) -> usize {
        self.by_address.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_address.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nocturne_snmp::Oid;

    fn bind(oid: &str, value: &str) -> VarBind {
        VarBind::new(
            oid.parse::<Oid>().expect("an oid"),
            Value::OctetString(value.as_bytes().to_vec()),
        )
    }

    fn text(bind: &VarBind) -> String {
        bind.value.display()
    }

    #[test]
    fn a_community_differs_by_one_letter_or_by_length_and_is_not_the_same() {
        assert!(same(b"n0cturne-ro", b"n0cturne-ro"));
        assert!(!same(b"n0cturne-ro", b"n0cturne-rw"));
        assert!(!same(b"n0cturne-ro", b"n0cturne-r"));
        assert!(!same(b"", b"x"));
        assert!(same(b"", b""));
    }

    #[test]
    fn a_forwarded_trap_does_not_bring_the_original_community_in_a_binding() {
        // What a trap proxy appends (RFC 3584 §3.1, item 4): the community
        // here is not the datagram's own, so only the object's definition
        // says what it is.
        let mut varbinds = vec![
            bind("1.3.6.1.6.3.18.1.4.0", "the-original-community"),
            bind("1.3.6.1.6.3.18.1.1.1.2.5", "another-one"),
            bind("1.3.6.1.2.1.1.5.0", "edge-sw-7"),
        ];
        scrub(&mut varbinds, b"n0cturne-ro");
        assert_eq!(text(&varbinds[0]), WITHHELD);
        assert_eq!(text(&varbinds[1]), WITHHELD);
        assert_eq!(text(&varbinds[2]), "edge-sw-7", "a name is not a community");
    }

    #[test]
    fn the_datagrams_own_community_is_cut_out_of_whatever_repeats_it() {
        let mut varbinds = vec![
            bind("1.3.6.1.4.1.99999.1.1.0", "n0cturne-ro"),
            bind(
                "1.3.6.1.4.1.99999.1.2.0",
                "snmp-server host 10.0.0.5 version 2c n0cturne-ro udp-port 1162",
            ),
            VarBind::new("1.3.6.1.2.1.2.2.1.1.3".parse().unwrap(), Value::Integer(3)),
        ];
        scrub(&mut varbinds, b"n0cturne-ro");
        assert_eq!(text(&varbinds[0]), WITHHELD);
        assert_eq!(
            text(&varbinds[1]),
            format!("snmp-server host 10.0.0.5 version 2c {WITHHELD} udp-port 1162")
        );
        assert_eq!(varbinds[2].value, Value::Integer(3));
    }

    #[test]
    fn a_two_letter_community_is_only_withheld_where_it_is_the_whole_value() {
        let mut varbinds = vec![
            bind("1.3.6.1.4.1.99999.1.1.0", "ro"),
            bind("1.3.6.1.2.1.1.1.0", "a router in the core"),
        ];
        scrub(&mut varbinds, b"ro");
        assert_eq!(text(&varbinds[0]), WITHHELD);
        assert_eq!(
            text(&varbinds[1]),
            "a router in the core",
            "two letters inside ordinary text are not a secret found"
        );
    }

    #[test]
    fn rejections_are_counted_per_address_and_never_more_than_the_cap() {
        let mut rejections = Rejections::default();
        for index in 0..1_000u32 {
            let address = IpAddr::from(std::net::Ipv4Addr::from(0x0a00_0000 + index));
            rejections.record(address, None, "unknown".into(), i64::from(index));
        }
        assert_eq!(rejections.len(), MAX_REJECTIONS);
        let recent = rejections.recent();
        assert_eq!(recent.len(), SHOWN_REJECTIONS);
        assert_eq!(recent[0].last_at, 999, "the newest is first");
        assert!(
            recent
                .iter()
                .all(|seen| seen.last_at >= 1_000 - MAX_REJECTIONS as i64),
            "the ones that made room were the oldest"
        );

        let again: IpAddr = "10.0.3.231".parse().unwrap();
        rejections.record(again, Some("dev1"), "the community is wrong".into(), 2_000);
        let recent = rejections.recent();
        assert_eq!(recent[0].address, "10.0.3.231");
        assert_eq!(
            recent[0].count, 2,
            "the same address is one row with a count"
        );
        assert_eq!(recent[0].device_id.as_deref(), Some("dev1"));
        assert_eq!(rejections.len(), MAX_REJECTIONS);
    }
}

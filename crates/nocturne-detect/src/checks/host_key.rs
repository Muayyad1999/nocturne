//! `host_key_changed`: a device presented an SSH host key that is not the
//! one remembered for it.

use serde_json::{Value, json};

use nocturne_app::model::{Device, Transport, collections};
use nocturne_app::{App, connect};

use super::compact;
use crate::DetectState;
use crate::catalogue::HOST_KEY_CHANGED;
use crate::model::{Finding, Severity};

/// How long the same key from the same host is the same finding. The
/// monitor and the backups reconnect all day, and the core announces the
/// mismatch on every attempt; one hit says it, the alert stays open, and a
/// day later it is worth saying again if it is still so.
const REPEAT_MS: i64 = 24 * 60 * 60_000;

/// The inventory devices a host and port belong to.
///
/// The verifier knows an address, not a device, and nothing stops two
/// devices from sharing one (a console server, a NAT). All of them are named
/// in the evidence; the hit is filed under a device only when there is
/// exactly one, because picking one of several would be a guess.
fn devices_at(app: &App, host: &str, port: u64) -> Vec<Device> {
    let devices: Vec<Device> = app.store().list(collections::DEVICES).unwrap_or_default();
    devices
        .into_iter()
        .filter(|device| device.transport == Transport::Ssh)
        .filter(|device| device.address.trim().eq_ignore_ascii_case(host.trim()))
        .filter(|device| u64::from(device.port.unwrap_or(22)) == port)
        .collect()
}

/// `connect.host_key_changed {host, port, known: {algorithm, fingerprint,
/// first_seen}, presented: {algorithm, fingerprint, openssh}}`.
pub fn check(app: &App, payload: &Value, at: i64) -> Option<Finding> {
    let host = payload["host"].as_str()?;
    let port = payload["port"].as_u64()?;
    let presented = payload["presented"]["fingerprint"].as_str()?;
    let devices = devices_at(app, host, port);
    let device = match devices.as_slice() {
        [only] => Some(only),
        _ => None,
    };
    let key = format!("{host}:{port}:{presented}");
    let seen_as = format!("{}:{key}", device.map_or("-", |device| device.id.as_str()));
    if !app
        .state::<DetectState>()
        .first_sighting(&seen_as, at, REPEAT_MS)
    {
        return None;
    }

    // With checking switched off the core announces the mismatch and then
    // connects anyway, which is the worse of the two outcomes and has to be
    // said: nothing was refused.
    let policy = connect::connection_settings(app).host_key_policy;
    let went_ahead = policy == "accept";
    let who = match device {
        Some(device) => device.name.clone(),
        None => format!("{host}:{port}"),
    };
    let title = if went_ahead {
        format!(
            "{who} presented a different SSH host key, and the connection went ahead because host key checking is off"
        )
    } else {
        format!("{who} presented a different SSH host key")
    };
    Some(Finding {
        detection: HOST_KEY_CHANGED,
        at,
        device_id: device.map(|device| device.id.clone()),
        severity: Severity::Critical,
        title,
        evidence: compact(json!({
            "decided_from": "connect.host_key_changed",
            "host": host,
            "port": port,
            "known": {
                "algorithm": payload["known"]["algorithm"],
                "fingerprint": payload["known"]["fingerprint"],
                "first_seen": payload["known"]["first_seen"],
            },
            "presented": {
                "algorithm": payload["presented"]["algorithm"],
                "fingerprint": presented,
            },
            "device_ids": devices.iter().map(|device| device.id.as_str()).collect::<Vec<_>>(),
            "host_key_policy": policy,
            "connection": if went_ahead { "went_ahead" } else { "refused" },
        })),
        key,
        detail: "Decided from the SSH connection itself: the key the host presented is not the one remembered for it. Either the device was replaced or re-keyed, or something is answering in its place.".into(),
    })
}

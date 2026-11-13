//! `logs.device_setup`: the lines that would point a device's syslog at this
//! Nocturne instance.
//!
//! It only writes them out. Nothing here touches a device: the lines are
//! meant to be handed to `changes.create`, which is the only thing in the
//! application that may configure anything.

use std::net::{IpAddr, SocketAddr, UdpSocket};

use nocturne_app::{App, AppError, AppResult, inventory, profiles};
use nocturne_drivers::hier::Syntax;
use serde::{Deserialize, Serialize};

use crate::settings::LogsSettings;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceSetup {
    pub device_id: String,
    pub profile: String,
    /// The configuration lines, in order. Not applied.
    pub lines: Vec<String>,
    /// The address the device is told to send to, and how it was worked out.
    pub address: String,
    pub address_note: String,
    pub udp_port: u16,
    pub note: String,
}

/// The address of ours that a packet to `device` would leave from.
///
/// A connected UDP socket makes the kernel pick the route and the source
/// address without sending anything, which is the only way to answer "which
/// of my addresses can that switch reach me on" on a multi-homed machine.
fn local_address_towards(address: &str) -> Option<IpAddr> {
    let peer: IpAddr = address.trim().parse().ok()?;
    let any = if peer.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = UdpSocket::bind(any).ok()?;
    socket.connect(SocketAddr::new(peer, 514)).ok()?;
    Some(socket.local_addr().ok()?.ip())
}

pub fn device_setup(app: &App, device_reference: &str) -> AppResult<DeviceSetup> {
    let device = inventory::find(app, device_reference)?;
    let state = inventory::state_of(app, &device.id)?;
    let profile_id = match (device.profile.as_str(), state.detected_profile.as_deref()) {
        ("", Some(detected)) => detected.to_string(),
        ("", None) => {
            return Err(AppError::invalid(format!(
                "`{}` has no device profile yet, so the lines it would need are unknown; \
                 connect to it once or set its profile",
                device.name
            )));
        }
        (profile, _) => profile.to_string(),
    };
    let platform = profiles::platform(app, &profile_id)?;
    let settings = LogsSettings::load(app);
    let listening = crate::receiver::listening(app);
    let port = crate::receiver::bound_port(app, "udp").unwrap_or(settings.syslog.udp_port);

    let (address, address_note) = match local_address_towards(&device.address) {
        Some(address) => (
            address.to_string(),
            format!(
                "the address on this machine that packets to {} leave from",
                device.address
            ),
        ),
        None => (
            "<this-nocturne-address>".to_string(),
            "replace this with the address of this machine as the device can reach it \
             (its own address could not be used to work one out)"
                .to_string(),
        ),
    };

    let lines = match platform.profile.config.syntax {
        Syntax::Ios => vec![
            "service timestamps log datetime msec show-timezone".to_string(),
            "logging origin-id hostname".to_string(),
            "logging trap informational".to_string(),
            if port == 514 {
                format!("logging host {address}")
            } else {
                format!("logging host {address} transport udp port {port}")
            },
        ],
        Syntax::Routeros => vec![
            format!(
                "/system logging action set [find name=remote] remote={address} remote-port={port}"
            ),
            "/system logging add topics=info action=remote".to_string(),
            "/system logging add topics=warning action=remote".to_string(),
            "/system logging add topics=error action=remote".to_string(),
            "/system logging add topics=critical action=remote".to_string(),
        ],
        _ => {
            return Err(AppError::new(
                "unsupported",
                format!(
                    "Nocturne does not know the syslog configuration of a {} device ({})",
                    platform.profile.name, platform.profile.id
                ),
            ));
        }
    };

    let mut note = String::from(
        "These lines are not applied: hand them to `changes.create` so they go through \
         the change pipeline like any other configuration.",
    );
    if !settings.syslog.enabled {
        note.push_str(" The syslog receiver is switched off, so nothing is listening yet.");
    } else if listening.is_empty() {
        note.push_str(
            " The receiver is enabled but not listening — see `logs.status` for why, \
             or the device will send into nothing.",
        );
    }
    if port != 514 {
        note.push_str(
            " The port is not the standard 514 because binding a port below 1024 needs \
             privileges this application does not ask for.",
        );
    }

    Ok(DeviceSetup {
        device_id: device.id,
        profile: profile_id,
        lines,
        address,
        address_note,
        udp_port: port,
        note,
    })
}

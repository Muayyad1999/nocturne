//! `logs.device_setup`: the lines that would point a device's syslog and its
//! SNMP notifications at this Nocturne instance.
//!
//! It only writes them out. Nothing here touches a device: the lines are
//! meant to be handed to `changes.create`, which is the only thing in the
//! application that may configure anything.
//!
//! **What does not belong here.** Which lines a platform needs is knowledge
//! about a device type, and a device type is data: it should be in the
//! platform's YAML profile. It is not. [`RECIPES`] holds it, keyed by
//! configuration syntax — which is all this module has ever keyed on, and
//! is too coarse: `syntax: ios` is also NX-OS, EOS and AOS-CX, whose
//! logging and SNMP commands are not these. The table is written as the
//! profile section it should become (line templates with `{address}`,
//! `{port}` and `{community}`, and whether the platform can be told a trap
//! port), so that moving it is a matter of reading it from the profile
//! instead; until then no platform is added to it.

use std::net::{IpAddr, SocketAddr, UdpSocket};

use nocturne_app::{App, AppError, AppResult, inventory, profiles};
use nocturne_drivers::hier::Syntax;
use serde::{Deserialize, Serialize};

use crate::settings::LogsSettings;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceSetup {
    pub device_id: String,
    pub profile: String,
    /// The configuration lines for syslog, in order. Not applied.
    pub lines: Vec<String>,
    /// The address the device is told to send to, and how it was worked out.
    pub address: String,
    pub address_note: String,
    pub udp_port: u16,
    pub note: String,
    /// The configuration lines for SNMP notifications, in order. Not
    /// applied, and not complete: where the community goes they say
    /// [`COMMUNITY_PLACEHOLDER`], because a community is a secret and is
    /// not put into anything this returns.
    pub trap_lines: Vec<String>,
    /// The port the trap receiver listens on (or would).
    pub trap_port: u16,
    pub trap_note: String,
}

/// What stands in a trap line where the community goes.
pub const COMMUNITY_PLACEHOLDER: &str = "<snmp-community>";

/// What one platform has to be told. See the module documentation: this is
/// a profile section that lives in the wrong place.
struct Recipe {
    syntax: Syntax,
    /// Syslog to any port: `{address}`, `{port}`.
    syslog: &'static [&'static str],
    /// Syslog to the standard port 514, where the platform writes that
    /// differently.
    syslog_standard: Option<&'static [&'static str]>,
    /// SNMP notifications: `{address}`, `{port}`, `{community}`.
    traps: &'static [&'static str],
    /// Whether the platform can be told which port to send notifications
    /// to. One that cannot sends them to 162.
    trap_port_settable: bool,
}

const RECIPES: &[Recipe] = &[
    Recipe {
        syntax: Syntax::Ios,
        syslog: &[
            "service timestamps log datetime msec show-timezone",
            "logging origin-id hostname",
            "logging trap informational",
            "logging host {address} transport udp port {port}",
        ],
        syslog_standard: Some(&[
            "service timestamps log datetime msec show-timezone",
            "logging origin-id hostname",
            "logging trap informational",
            "logging host {address}",
        ]),
        traps: &[
            "snmp-server enable traps snmp authentication linkdown linkup coldstart warmstart",
            "snmp-server host {address} version 2c {community} udp-port {port}",
        ],
        trap_port_settable: true,
    },
    Recipe {
        syntax: Syntax::Routeros,
        syslog: &[
            "/system logging action set [find name=remote] remote={address} remote-port={port}",
            "/system logging add topics=info action=remote",
            "/system logging add topics=warning action=remote",
            "/system logging add topics=error action=remote",
            "/system logging add topics=critical action=remote",
        ],
        syslog_standard: None,
        // RouterOS's `/snmp` has `trap-target`, `trap-community`,
        // `trap-version`, `trap-generators` and `trap-interfaces`, and no
        // port (MikroTik's SNMP page, help.mikrotik.com/docs/spaces/ROS/pages/8978519/SNMP).
        traps: &[
            "/snmp set trap-version=2 trap-community={community} trap-target={address} trap-generators=interfaces,start-trap trap-interfaces=all",
        ],
        trap_port_settable: false,
    },
];

fn filled(templates: &[&str], address: &str, port: u16) -> Vec<String> {
    templates
        .iter()
        .map(|line| {
            line.replace("{address}", address)
                .replace("{port}", &port.to_string())
                .replace("{community}", COMMUNITY_PLACEHOLDER)
        })
        .collect()
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
    let listening = crate::syslog::listening(app);
    let port = crate::syslog::bound_port(app, "udp").unwrap_or(settings.syslog.udp_port);

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

    let Some(recipe) = RECIPES
        .iter()
        .find(|recipe| recipe.syntax == platform.profile.config.syntax)
    else {
        return Err(AppError::new(
            "unsupported",
            format!(
                "Nocturne does not know the syslog configuration of a {} device ({})",
                platform.profile.name, platform.profile.id
            ),
        ));
    };
    let lines = filled(
        recipe
            .syslog_standard
            .filter(|_| port == 514)
            .unwrap_or(recipe.syslog),
        &address,
        port,
    );
    let trap_port = crate::traps::bound_port(app).unwrap_or(settings.traps.port);
    let trap_lines = filled(recipe.traps, &address, trap_port);
    let trap_note = trap_note(app, &device, &settings, recipe, trap_port);

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
        trap_lines,
        trap_port,
        trap_note,
    })
}

/// What has to be said beside the trap lines: where the community comes
/// from, whether the receiver would keep what the device then sends, and
/// whether anything is listening where it would send it.
fn trap_note(
    app: &App,
    device: &nocturne_app::model::Device,
    settings: &LogsSettings,
    recipe: &Recipe,
    port: u16,
) -> String {
    use crate::traps::admit::{self, Known};

    let mut note = format!(
        "These lines are not applied, and not complete. Put the SNMP community of this device \
         where they say {COMMUNITY_PLACEHOLDER}: a community is a secret, and Nocturne does not \
         put one into a line it returns. Then hand them to `changes.create`."
    );
    let known = admit::lookup(app, device);
    if settings.traps.require_known_community {
        match &known {
            Known::Community(_) => note.push_str(
                " The receiver keeps a notification from this device only when it carries \
                 exactly the community stored in its SNMP credential, so that is the one to use.",
            ),
            Known::User(_) => note.push_str(
                " As things stand what these lines make the device send would be rejected: its \
                 SNMP credential is an SNMPv3 user, and these lines send SNMPv2c. Store an \
                 SNMPv2c community for it, or switch `require_known_community` off.",
            ),
            Known::Nothing(why) => note.push_str(&format!(
                " As things stand what these lines make the device send would be rejected: {why}. \
                 Store an SNMPv2c community for it, or switch `require_known_community` off."
            )),
        }
    } else {
        note.push_str(
            " `require_known_community` is off, so any community is accepted from this device.",
        );
    }
    note.push_str(
        " SNMPv2c is unauthenticated UDP: whoever can reach the port can forge a notification, \
         and its community travels in the clear.",
    );
    if !settings.traps.enabled {
        note.push_str(" The trap receiver is switched off, so nothing is listening yet.");
    } else if crate::traps::listening(app).is_empty() {
        note.push_str(
            " The trap receiver is enabled but not listening — see `logs.status` for why, \
             or the device will send into nothing.",
        );
    }
    if port != 162 {
        if recipe.trap_port_settable {
            note.push_str(
                " The port is not the standard 162 because binding a port below 1024 needs \
                 privileges this application does not ask for.",
            );
        } else {
            note.push_str(&format!(
                " This platform has no setting for the port notifications are sent to: it sends \
                 them to UDP 162, and this receiver listens on {port}. They will not arrive unless \
                 the receiver is given port 162 (which needs privileges) or this machine forwards \
                 162 to {port}."
            ));
        }
    }
    note
}

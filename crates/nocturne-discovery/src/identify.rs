//! What answered, and how we know.
//!
//! One address at a time: echo, the ports the scan was told to try, SNMP with
//! the credentials the user named, the SSH identification string the server
//! volunteered, reverse DNS, and the MAC from this machine's own neighbour
//! table with its registered vendor. Every step may fail, and a failed step
//! leaves its field empty — never a default that reads like an answer.
//!
//! The device type is then asked of the **profile registry**, which is where
//! device types live: `detect::from_snmp` against `sysDescr`/`sysObjectID`,
//! then `detect::from_banner` against the SSH identification string. What
//! this module refuses to do, deliberately:
//!
//! * **guess a platform from open ports.** Port 8291 is a strong hint of one
//!   vendor and no proof of anything; if no profile's `detect:` rules match,
//!   the find has no profile and says so.
//! * **guess a platform from the MAC vendor.** An OUI says who made the
//!   network card.
//! * **guess a role.** Nothing in a port list says "this is a core switch".
//!   A role is only ever suggested when a device Nocturne already manages
//!   reported one (crawl), and then it is the neighbour's own word.
//! * **turn reverse DNS into a system name, or the other way round.**
//!   `hostname` and `snmp.sys_name` are different claims by different
//!   parties; a find can have either, both, or neither.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use nocturne_app::model::{Device, collections::DEVICES};
use nocturne_app::{App, AppResult, collect, profiles, util};
use nocturne_drivers::detect;
use nocturne_drivers::model::{Interface, Kind};
use nocturne_netprobe::local::Neighbour;

use crate::model::{Found, OpenPort, ProfileFind, SnmpFind, Suggested};
use crate::probe::{Probes, Rate};
use crate::snmp::Credential;

/// Everything one scan's identification step needs, shared between its tasks.
pub struct Context {
    pub probes: Arc<dyn Probes>,
    pub rate: Arc<Rate>,
    pub timeout: Duration,
    pub ports: Vec<u16>,
    pub icmp: bool,
    pub identify_ssh: bool,
    pub resolve_names: bool,
    pub snmp: Arc<Vec<Credential>>,
    pub snmp_port: u16,
}

/// The SSH port, where an identification string is read without logging in.
pub const SSH_PORT: u16 = 22;
/// The Telnet port, which decides the suggested transport when SSH is closed.
pub const TELNET_PORT: u16 = 23;
/// How long a reverse lookup may take.
const DNS_TIMEOUT: Duration = Duration::from_millis(1500);

/// Probe one address. `None` when nothing answered at all.
pub async fn probe_address(context: &Context, address: IpAddr) -> Option<Found> {
    let mut found = Found {
        address: address.to_string(),
        found_at: util::now_ms(),
        ..Found::default()
    };

    let echo = if context.icmp {
        context.rate.permit().await;
        context.probes.echo(address, context.timeout).await
    } else {
        None
    };
    if let Some(rtt) = echo {
        found.alive = true;
        found.via = "icmp".into();
        found.rtt_ms = Some(rtt);
        found.say(
            "alive",
            "icmp",
            format!("answered an echo request in {rtt:.1} ms"),
        );
    }

    // One permit for each TCP connection the port check is about to open.
    for _ in &context.ports {
        context.rate.permit().await;
    }
    let answers = context
        .probes
        .ports(
            address,
            &context.ports,
            context.timeout,
            context.identify_ssh,
        )
        .await;
    for answer in answers.into_iter().filter(|answer| answer.open) {
        if !found.alive {
            found.alive = true;
            found.via = format!("tcp/{}", answer.port);
            found.rtt_ms = answer.rtt_ms;
            found.say(
                "alive",
                &found.via.clone(),
                format!("accepted a TCP connection on port {}", answer.port),
            );
        }
        if answer.port == SSH_PORT
            && let Some(banner) = &answer.banner
            && banner.starts_with("SSH-")
        {
            found.ssh_banner = Some(banner.clone());
            found.say(
                "ssh_banner",
                "tcp/22",
                format!("the server volunteered `{banner}` before any login"),
            );
        }
        found.say(
            "open_ports",
            &format!("tcp/{}", answer.port),
            match &answer.banner {
                Some(banner) => format!("port {} is open and said `{banner}`", answer.port),
                None => format!("port {} is open", answer.port),
            },
        );
        found.open_ports.push(OpenPort {
            port: answer.port,
            service: answer.service,
            banner: answer.banner,
        });
    }

    if !found.alive {
        return None;
    }

    identify(context, address, &mut found).await;
    Some(found)
}

/// The identification half: SNMP, names, MAC, device type. Used by the sweep
/// and by the crawl, which gets here with a candidate that is already known
/// to exist because another device said so.
pub async fn identify(context: &Context, address: IpAddr, found: &mut Found) {
    for credential in context.snmp.iter() {
        context.rate.permit().await;
        let target = SocketAddr::new(address, context.snmp_port);
        match context
            .probes
            .snmp(target, credential.auth.clone(), context.timeout)
            .await
        {
            Ok(info) if !info.is_empty() => {
                found.say(
                    "snmp",
                    "snmp",
                    format!(
                        "the agent answered the credential `{}`{}",
                        credential.name,
                        match &info.sys_descr {
                            Some(descr) => format!(" with sysDescr `{}`", clip(descr)),
                            None => String::new(),
                        }
                    ),
                );
                found.snmp = Some(SnmpFind {
                    credential_id: credential.id.clone(),
                    credential_name: credential.name.clone(),
                    sys_name: info.sys_name,
                    sys_descr: info.sys_descr,
                    sys_object_id: info.sys_object_id,
                    uptime_s: info.uptime_s,
                });
                if !found.alive {
                    found.alive = true;
                    found.via = "snmp".into();
                    found.say(
                        "alive",
                        "snmp",
                        "answered SNMP although it answered no echo request and no port",
                    );
                }
                break;
            }
            // An agent that answers nothing and an agent that is not there are
            // the same thing to the next credential in the list.
            Ok(_) => {}
            Err(error) => tracing::trace!(%address, %error, "no SNMP answer"),
        }
    }

    if context.resolve_names {
        found.hostname = context.probes.reverse_dns(address, DNS_TIMEOUT).await;
        if let Some(name) = &found.hostname {
            found.say("hostname", "dns", format!("reverse DNS says `{name}`"));
        }
    }
}

/// Attach the MAC and its vendor from this machine's neighbour table.
///
/// Only hosts on a segment this machine is itself attached to have an entry,
/// which is exactly what the field means: nothing is inferred for the rest.
pub fn attach_mac(found: &mut Found, neighbours: &HashMap<String, Neighbour>) {
    let Some(neighbour) = neighbours.get(&found.address) else {
        return;
    };
    found.mac = Some(neighbour.mac.clone());
    found.say(
        "mac",
        "arp",
        format!(
            "this machine's neighbour table has {} at {} on {}",
            found.address, neighbour.mac, neighbour.interface
        ),
    );
    if let Some(vendor) = nocturne_tools::mac::vendor(&neighbour.mac) {
        found.say(
            "vendor",
            "oui",
            format!("the OUI registry assigns {} to {vendor}", neighbour.mac),
        );
        found.vendor = Some(vendor);
    }
}

/// Ask the profile registry what this is.
///
/// SNMP first (`sysDescr` and `sysObjectID` are what a device says about
/// itself), then the SSH identification string. No match is a result, not a
/// failure: the terminal still works with the `generic` profile and the user
/// can say what it is.
pub fn match_profile(app: &App, found: &mut Found) {
    let registry = profiles::registry(app);
    let detection = found
        .snmp
        .as_ref()
        .and_then(|snmp| {
            detect::from_snmp(
                &registry,
                snmp.sys_descr.as_deref().unwrap_or_default(),
                snmp.sys_object_id.as_deref().unwrap_or_default(),
            )
        })
        .or_else(|| {
            found
                .ssh_banner
                .as_deref()
                .and_then(|banner| detect::from_banner(&registry, banner))
        });
    let Some(detection) = detection else {
        return;
    };
    let name = registry
        .get(&detection.profile_id)
        .map(|platform| platform.profile.name.clone())
        .unwrap_or_else(|| detection.profile_id.clone());
    let method = match detection.method {
        detect::DetectMethod::Snmp => "snmp",
        detect::DetectMethod::Banner => "banner",
        detect::DetectMethod::Probe => "probe",
    };
    found.say(
        "profile",
        method,
        format!(
            "the device type `{}` matched on {}",
            detection.profile_id, detection.evidence
        ),
    );
    found.profile = Some(ProfileFind {
        id: detection.profile_id,
        name,
        method: method.to_string(),
        evidence: detection.evidence,
    });
}

/// What `discovery.add` would create, and why.
///
/// The name is the device's own (`sysName`), else the short form of its
/// reverse-DNS name, else the address — in that order, because that is the
/// order of how much the device itself had to do with the answer.
pub fn suggest(found: &mut Found, site: &str, role: &str) {
    let (name, source, detail) = match found
        .snmp
        .as_ref()
        .and_then(|snmp| snmp.sys_name.clone())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
    {
        Some(name) => (
            name.clone(),
            "snmp",
            format!("the device calls itself `{name}` (sysName)"),
        ),
        None => match found
            .hostname
            .as_deref()
            .map(short_name)
            .filter(|name| !name.is_empty())
        {
            Some(name) => (
                name.clone(),
                "dns",
                format!("named after its reverse-DNS name `{name}`"),
            ),
            None => (
                found.address.clone(),
                "icmp",
                "nothing named it, so the address is the name".to_string(),
            ),
        },
    };
    found.say("suggested.name", source, detail);

    let open = |port: u16| found.open_ports.iter().any(|entry| entry.port == port);
    let (transport, reason) = if open(SSH_PORT) {
        ("ssh", "port 22 is open")
    } else if open(TELNET_PORT) {
        ("telnet", "port 22 is closed and port 23 is open")
    } else {
        (
            "none",
            "neither port 22 nor port 23 is open, so Nocturne would only monitor it",
        )
    };
    found.say("suggested.transport", "tcp", reason);

    if !role.is_empty()
        && let Some(told) = found.told_by.clone()
    {
        found.say(
            "suggested.role",
            "neighbor",
            format!(
                "`{}` reports this neighbour's capabilities as `{}`",
                told.device_name, told.capabilities
            ),
        );
    }

    found.suggested = Suggested {
        name,
        transport: transport.to_string(),
        role: role.to_string(),
        profile: found
            .profile
            .as_ref()
            .map(|profile| profile.id.clone())
            .unwrap_or_default(),
        site: site.to_string(),
        snmp_credential_id: found.snmp.as_ref().map(|snmp| snmp.credential_id.clone()),
    };
}

/// The first label of a DNS name: `sw1.campus.example` → `sw1`.
pub fn short_name(name: &str) -> String {
    name.trim_end_matches('.')
        .split('.')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Addresses already in the inventory: the device's own address, and every
/// address on an interface Nocturne has collected from it.
pub struct Known {
    by_address: HashMap<String, (String, String)>,
}

impl Known {
    pub fn load(app: &App) -> AppResult<Known> {
        let mut by_address = HashMap::new();
        let devices: Vec<Device> = app.store().list(DEVICES)?;
        for device in &devices {
            if let Some(address) = normalise(&device.address) {
                by_address.insert(address, (device.id.clone(), device.name.clone()));
            }
        }
        // A device is often managed by one address and found by another: the
        // addresses of its interfaces are the same device, not a new find.
        for stored in collect::latest_all(app, Kind::Interfaces)? {
            let Some(device) = devices
                .iter()
                .find(|device| device.id == stored.device_id)
                .map(|device| (device.id.clone(), device.name.clone()))
            else {
                continue;
            };
            for interface in stored.typed::<Interface>() {
                for cidr in interface.ipv4.iter().chain(interface.ipv6.iter()) {
                    let address = cidr.split('/').next().unwrap_or_default();
                    if let Some(address) = normalise(address) {
                        by_address.entry(address).or_insert_with(|| device.clone());
                    }
                }
            }
        }
        Ok(Known { by_address })
    }

    pub fn get(&self, address: &str) -> Option<&(String, String)> {
        normalise(address).and_then(|address| self.by_address.get(&address))
    }

    /// Mark a find that Nocturne already manages.
    pub fn mark(&self, found: &mut Found) {
        if let Some((id, name)) = self.get(&found.address) {
            found.device_id = Some(id.clone());
            found.device_name = Some(name.clone());
            found.say(
                "device_id",
                "inventory",
                format!("`{name}` is already in the inventory with this address"),
            );
        }
    }
}

/// An address in a comparable form; `None` for a name (devices may be managed
/// by name, and a name is not an address a sweep can match).
fn normalise(address: &str) -> Option<String> {
    address
        .trim()
        .parse::<IpAddr>()
        .ok()
        .map(|ip| ip.to_string())
}

fn clip(text: &str) -> String {
    let text = text.trim().replace(['\n', '\r'], " ");
    if text.chars().count() <= 120 {
        return text;
    }
    text.chars().take(117).collect::<String>() + "…"
}

//! The shapes discovery puts on the wire.
//!
//! Two of them carry the feature's whole argument. [`Found`] is what one
//! address answered — every field is something a probe *said*, and nothing is
//! filled in to make the record look complete. [`Evidence`] says, field by
//! field, which probe said it, so a person reading a proposal can see why
//! Nocturne thinks what it thinks and throw it out when the reason is thin.

use serde::{Deserialize, Serialize};

/// What a scan was asked to do.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct ScanRequest {
    /// `"10.0.0.0/24, 10.0.1.5-10.0.1.40, core-sw.example"`.
    pub targets: String,
    /// Send an echo request to every address.
    pub icmp: bool,
    /// TCP ports to try on every address. Absent (or `null`) = the ports in
    /// the settings; `""` = none. The two have to be told apart, which a
    /// plain string cannot do, so this is an option.
    pub ports: Option<String>,
    /// Credentials holding communities / v3 users, tried in this order. Only
    /// these are ever used: discovery never tries a credential it was not given.
    pub snmp_credential_ids: Vec<String>,
    /// Read the SSH identification string that port 22 volunteers. Never a login.
    pub identify_ssh: bool,
    /// Reverse-resolve the addresses that answered.
    pub resolve_names: bool,
    /// The most addresses this scan may cover (bounded by the settings).
    pub max_hosts: u32,
    /// Probes per second across the whole scan (bounded by the settings).
    pub rate: u32,
    /// Probes in flight at once (bounded by the settings).
    pub concurrency: u32,
    /// Per-probe timeout in milliseconds (bounded by the settings).
    pub timeout_ms: u64,
    /// The UDP port SNMP is asked on.
    pub snmp_port: u16,
    /// Sweep addresses that are not private, link-local or loopback. Refused
    /// unless the settings allow it, and written into the job log when used.
    pub allow_public: bool,
    /// A label for the scan, for the person reading the list later.
    pub note: String,
}

impl Default for ScanRequest {
    fn default() -> Self {
        ScanRequest {
            targets: String::new(),
            icmp: true,
            ports: None,
            snmp_credential_ids: Vec::new(),
            identify_ssh: true,
            resolve_names: true,
            max_hosts: 0,
            rate: 0,
            concurrency: 0,
            timeout_ms: 0,
            snmp_port: 161,
            allow_public: false,
            note: String::new(),
        }
    }
}

/// An open TCP port and what, if anything, it said first.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenPort {
    pub port: u16,
    /// The service usually found on this port, from `nocturne-netprobe`'s
    /// table. It is what the port *number* means, never what the host is.
    pub service: String,
    pub banner: Option<String>,
}

/// What an SNMP agent said about itself.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SnmpFind {
    /// The credential that answered — never the community or passphrase itself.
    pub credential_id: String,
    pub credential_name: String,
    pub sys_name: Option<String>,
    pub sys_descr: Option<String>,
    pub sys_object_id: Option<String>,
    pub uptime_s: Option<u64>,
}

/// The device type the profile registry matched, and what matched it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileFind {
    pub id: String,
    pub name: String,
    /// `snmp` or `banner`: which of the registry's detect rules fired.
    pub method: String,
    /// The text that matched, as the registry clipped it.
    pub evidence: String,
}

/// Why a field of a [`Found`] says what it says.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Evidence {
    /// The field of [`Found`] (or of [`Suggested`], as `suggested.name`).
    pub field: String,
    /// `icmp`, `tcp/22`, `arp`, `dns`, `snmp`, `oui`, `profile`, `neighbor`,
    /// `inventory` — the probe or record that is the source of the value.
    pub source: String,
    /// One sentence a person can check.
    pub detail: String,
}

impl Evidence {
    pub fn new(field: &str, source: &str, detail: impl Into<String>) -> Evidence {
        Evidence {
            field: field.to_string(),
            source: source.to_string(),
            detail: detail.into(),
        }
    }
}

/// The device `discovery.add` would create from a find. A proposal, not a fact.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Suggested {
    pub name: String,
    /// `ssh`, `telnet` or `none` — from which port is open, nothing else.
    pub transport: String,
    /// Empty unless a device Nocturne already manages said what this is.
    pub role: String,
    /// The profile id the registry matched, empty when nothing matched.
    pub profile: String,
    pub site: String,
    /// The SNMP credential that answered, so monitoring can be switched on with it.
    pub snmp_credential_id: Option<String>,
}

/// Which device in the inventory told us about a crawled find.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToldBy {
    pub device_id: String,
    pub device_name: String,
    /// `cdp`, `lldp`, `arp`, `route`, … — the table the address came out of.
    pub protocol: String,
    /// The seed's own interface, when the neighbour table named one.
    pub local_interface: String,
    /// The neighbour's interface, when the neighbour table named one.
    pub remote_interface: String,
    /// The name the neighbour gave itself in that table.
    pub reported_name: String,
    /// The capabilities the seed reported for it (`Router`, `Switch`, …), as
    /// the seed spelled them. The only honest source of a role suggestion.
    pub capabilities: String,
}

/// One address that answered something.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Found {
    pub address: String,
    pub alive: bool,
    pub rtt_ms: Option<f64>,
    /// What proved it alive: `icmp`, `tcp/<port>`, `arp`, or `neighbor` for a
    /// crawl candidate that is only known from another device's table.
    pub via: String,
    /// Reverse DNS. Not the same thing as `snmp.sys_name`, and never guessed
    /// from one when the other is missing.
    pub hostname: Option<String>,
    /// From this machine's neighbour table: only hosts on a local segment have one.
    pub mac: Option<String>,
    /// The MAC's registered organisation, from the OUI registry.
    pub vendor: Option<String>,
    pub open_ports: Vec<OpenPort>,
    pub snmp: Option<SnmpFind>,
    pub ssh_banner: Option<String>,
    /// The device type, when the profile registry matched one. No match means
    /// no profile: a guess here would be a lie with consequences.
    pub profile: Option<ProfileFind>,
    pub suggested: Suggested,
    /// Set when this address is already in the inventory.
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub ignored: bool,
    /// Devices already in the inventory that led here, seed first (crawl only).
    pub path: Vec<String>,
    pub told_by: Option<ToldBy>,
    pub found_at: i64,
    /// Why every field above says what it says.
    pub evidence: Vec<Evidence>,
}

impl Found {
    pub fn say(&mut self, field: &str, source: &str, detail: impl Into<String>) {
        self.evidence.push(Evidence::new(field, source, detail));
    }

    /// Whether anything identified this as a device of a known type.
    pub fn identified(&self) -> bool {
        self.profile.is_some()
    }
}

/// A scan as the list of scans shows it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scan {
    pub id: String,
    /// `sweep` or `crawl`.
    pub kind: String,
    /// The target expression as typed, or the seed devices for a crawl.
    pub targets: String,
    pub note: String,
    pub job_id: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    /// `running`, `finished`, `cancelled` or `failed`.
    pub state: String,
    pub error: Option<String>,
    /// Addresses the scan was asked to probe.
    pub planned: u32,
    /// Addresses probed so far.
    pub probed: u32,
    /// Addresses that answered something.
    pub found: u32,
    /// Finds the profile registry could put a device type to.
    pub identified: u32,
    /// Finds that are not already in the inventory.
    pub new: u32,
    /// Probes actually sent (echo, TCP connect, SNMP get).
    pub probes_sent: u64,
    /// Whether public address space was swept, and said so in the job log.
    pub allow_public: bool,
}

/// An address the user never wants offered again.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ignored {
    pub address: String,
    pub note: String,
    pub at: i64,
}

/// One of this machine's own connected networks, offered as a starting point.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SuggestedNetwork {
    pub cidr: String,
    pub interface: String,
    /// Host addresses in the block (what a sweep of it would cover).
    pub hosts: u64,
    /// This machine's own address on that network.
    pub address: String,
    /// Too large to sweep under the current settings.
    pub too_large: bool,
    /// Outside private, link-local and loopback space: a sweep of it needs
    /// `allow_public` and the installation's permission. A workstation can
    /// perfectly well sit on public space the operator owns, so the network
    /// is still offered — with this said plainly.
    pub needs_allow_public: bool,
}

/// A default route of this machine.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SuggestedGateway {
    pub address: String,
    pub interface: String,
    pub destination: String,
}

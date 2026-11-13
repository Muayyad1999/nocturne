//! Domain types shared by several feature modules.

use nocturne_drivers::model::Facts;
use serde::{Deserialize, Serialize};

/// Names of the document collections, in one place.
pub mod collections {
    pub const DEVICES: &str = "devices";
    pub const DEVICE_STATE: &str = "device_state";
    pub const CREDENTIALS: &str = "credentials";
}

/// How the CLI of a device is reached.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    #[default]
    Ssh,
    Telnet,
    /// A serial console; `address` is the port name (`COM3`, `/dev/ttyUSB0`).
    Serial,
    /// A simulated device in a lab.
    Sim,
    /// No CLI access: the device is only monitored (ICMP / SNMP).
    None,
}

/// A device under management.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Device {
    pub id: String,
    /// Display name; unique, case-insensitively.
    pub name: String,
    /// Hostname or IP address (or serial port for `Transport::Serial`).
    pub address: String,
    /// Device profile id (`cisco_ios`, …). Empty = detect on first connection.
    pub profile: String,
    pub transport: Transport,
    /// TCP port (or baud rate for serial); `None` = the transport's default.
    pub port: Option<u32>,
    pub credential_id: Option<String>,
    /// Reach this device through another one (SSH jump host).
    pub jump_device_id: Option<String>,
    pub site: String,
    /// `core`, `distribution`, `access`, `edge`, `firewall`, `wireless`, `server`, … (free text).
    pub role: String,
    pub tags: Vec<String>,
    pub notes: String,
    pub monitor: MonitorSpec,
    /// Set for simulated devices.
    pub lab: Option<LabRef>,
    /// Protected devices are never changed by automation or the AI without an
    /// explicit, per-change human approval — whatever the autonomy setting.
    pub protected: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MonitorSpec {
    pub enabled: bool,
    /// Reachability and latency by ICMP echo.
    pub icmp: bool,
    /// Interface counters and health over SNMP.
    pub snmp: bool,
    /// Credential holding the SNMP community / v3 user (defaults to the device credential).
    pub snmp_credential_id: Option<String>,
    pub snmp_port: Option<u16>,
    /// Seconds between polls.
    pub interval_s: u32,
}

impl Default for MonitorSpec {
    fn default() -> Self {
        MonitorSpec {
            enabled: true,
            icmp: true,
            snmp: false,
            snmp_credential_id: None,
            snmp_port: None,
            interval_s: 60,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LabRef {
    pub lab_id: String,
    /// Node name inside the lab.
    pub node: String,
}

/// What Nocturne currently knows about a device, updated by monitoring and collection.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceState {
    pub device_id: String,
    /// `None` until the first check.
    pub reachable: Option<bool>,
    pub rtt_ms: Option<f64>,
    pub last_seen: Option<i64>,
    pub last_checked: Option<i64>,
    pub last_error: Option<String>,
    pub facts: Option<Facts>,
    pub facts_at: Option<i64>,
    /// Profile chosen by auto-detection when the device's own is empty.
    pub detected_profile: Option<String>,
}

/// A device together with its state, as lists show it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceRow {
    #[serde(flatten)]
    pub device: Device,
    pub state: DeviceState,
}

/// A stored login. Secrets live in the vault and are never part of this record;
/// the `has_*` flags say which ones are set.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Credential {
    pub id: String,
    pub name: String,
    pub username: String,
    pub has_password: bool,
    pub has_enable_secret: bool,
    pub has_private_key: bool,
    pub has_passphrase: bool,
    pub snmp: SnmpAccess,
    pub notes: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// The non-secret half of SNMP access.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SnmpAccess {
    /// `""` (none), `v1`, `v2c` or `v3`.
    pub version: String,
    pub has_community: bool,
    /// SNMPv3 user name.
    pub user: String,
    /// `md5`, `sha1`, `sha224`, `sha256`, `sha384`, `sha512` or empty.
    pub auth_protocol: String,
    pub has_auth_password: bool,
    /// `des`, `aes128`, `aes192`, `aes256` or empty.
    pub priv_protocol: String,
    pub has_priv_password: bool,
}

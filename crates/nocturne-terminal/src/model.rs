//! What crosses the RPC boundary: session descriptions, the ad-hoc target of
//! `terminal.open`, and event payloads.

use nocturne_transport::{ConnectionInfo, FlowControl, Parity, TransportKind};
use serde::{Deserialize, Serialize};

/// The wire name of a transport kind.
pub(crate) fn kind_name(kind: TransportKind) -> &'static str {
    match kind {
        TransportKind::Ssh => "ssh",
        TransportKind::Telnet => "telnet",
        TransportKind::Serial => "serial",
        TransportKind::Local => "local",
        TransportKind::Sim => "sim",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Connecting,
    Open,
    Closed,
}

/// A terminal session as the UI sees it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    /// `SW1`, `admin@10.0.0.1`, `COM3`, `Local shell`.
    pub title: String,
    pub kind: TransportKind,
    pub device_id: Option<String>,
    /// The device is simulated: whatever happens here happens in a lab.
    pub lab: bool,
    pub opened_at: i64,
    pub state: SessionState,
    pub close_reason: Option<String>,
    pub closed_at: Option<i64>,
    pub cols: u16,
    pub rows: u16,
    pub info: ConnectionInfo,
    /// Bytes received from the far end.
    pub bytes_in: u64,
    /// Bytes sent to the far end.
    pub bytes_out: u64,
    /// Name of the log file the session is (or, once closed, was) written to.
    pub logging: Option<String>,
    /// Why the session is not logged although it was meant to be.
    pub log_error: Option<String>,
}

/// A target that is not in the inventory.
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub struct Adhoc {
    pub transport: AdhocTransport,
    /// Host name or address; for `serial` the port name (`COM3`, `/dev/ttyUSB0`).
    pub host: String,
    /// `None` or `0`: the transport's default.
    pub port: Option<u16>,
    pub username: String,
    /// Used for this session only; never stored or logged.
    pub password: Option<String>,
    /// Take user name and secrets from a stored credential instead.
    pub credential_id: Option<String>,
    pub jump_device_id: Option<String>,
    pub serial: SerialOptions,
    pub local: LocalOptions,
    pub title: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdhocTransport {
    #[default]
    Ssh,
    Telnet,
    Serial,
    Local,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SerialOptions {
    pub baud: u32,
    pub data_bits: u8,
    pub parity: Parity,
    pub stop_bits: u8,
    pub flow: FlowControl,
}

impl Default for SerialOptions {
    /// 9600 8N1 without flow control: what a network console expects.
    fn default() -> Self {
        SerialOptions {
            baud: 9600,
            data_bits: 8,
            parity: Parity::None,
            stop_bits: 1,
            flow: FlowControl::None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalOptions {
    /// The user's shell when `None`.
    pub program: Option<String>,
    pub args: Vec<String>,
    /// The user's home directory when `None`.
    pub cwd: Option<String>,
}

/// Payload of `terminal.output`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutputEvent {
    pub session_id: String,
    /// Counts the output events of the session from 1, without gaps.
    pub seq: u64,
    /// Base64 of the raw bytes.
    pub data: String,
}

/// Payload of `terminal.closed`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClosedEvent {
    pub session_id: String,
    pub reason: String,
}

/// Payload of `terminal.removed`: the backend no longer knows the session.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemovedEvent {
    pub session_id: String,
}

/// Result of `terminal.attach`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attached {
    pub info: SessionInfo,
    /// Base64 of the retained output.
    pub scrollback: String,
    /// The last output event the scrollback includes.
    pub seq: u64,
}

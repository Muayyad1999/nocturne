//! The `toolbox` settings section: the defaults a probe starts from and the
//! bounds it is held to.
//!
//! The calculators need nothing from here — they are arithmetic. Everything
//! below is about packets leaving this machine, or a socket being bound on it,
//! so every number is clamped on read: a typo in the settings screen cannot
//! make a ping run for a week or a port scan open ten thousand sockets.
//!
//! The server ports default to unprivileged ones (6969 for TFTP, 5515 for the
//! staging syslog listener — deliberately not 5514, which the `logs` feature's
//! own receiver takes) because binding below 1024 needs rights the application
//! does not have and should not want.

use nocturne_app::App;
use serde::{Deserialize, Serialize};

pub const SECTION: &str = "toolbox";

/// Hard ceiling on how long one continuous ping may run.
pub const MAX_RUN_S: u64 = 3600;
/// Hard ceiling on the addresses one sweep may cover.
pub const MAX_SWEEP_HOSTS: u32 = 65_536;
/// Hard ceiling on the ports one scan may try.
pub const MAX_PORTS: u32 = 65_535;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolboxSettings {
    pub probe: ProbeSettings,
    pub sweep: SweepSettings,
    pub tftp: TftpSettings,
    pub syslog: SyslogSettings,
    /// How many finished probe runs are kept in the history.
    pub keep_runs: u32,
}

impl Default for ToolboxSettings {
    fn default() -> Self {
        ToolboxSettings {
            probe: ProbeSettings::default(),
            sweep: SweepSettings::default(),
            tftp: TftpSettings::default(),
            syslog: SyslogSettings::default(),
            keep_runs: 200,
        }
    }
}

/// What a single-target probe does when the request does not say.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProbeSettings {
    pub ping_count: u32,
    pub ping_interval_ms: u64,
    pub ping_timeout_ms: u64,
    pub ping_payload: usize,
    /// How long a ping with `count: 0` (until cancelled) may run.
    pub max_run_s: u64,
    pub trace_max_hops: u8,
    pub trace_probes: u8,
    pub trace_timeout_ms: u64,
    /// Ports a port check tries when the request names none.
    pub ports: String,
    pub port_timeout_ms: u64,
    pub port_concurrency: usize,
    /// The most ports one check may try.
    pub max_ports: u32,
    pub dns_timeout_ms: u64,
    pub whois_timeout_ms: u64,
    pub tls_timeout_ms: u64,
    pub http_timeout_ms: u64,
    pub ntp_timeout_ms: u64,
    /// Look up the names of the routers a traceroute finds.
    pub resolve_names: bool,
}

impl Default for ProbeSettings {
    fn default() -> Self {
        ProbeSettings {
            ping_count: 4,
            ping_interval_ms: 1000,
            ping_timeout_ms: 1000,
            ping_payload: 56,
            max_run_s: MAX_RUN_S,
            trace_max_hops: 30,
            trace_probes: 3,
            trace_timeout_ms: 2000,
            ports: "common".into(),
            port_timeout_ms: 1000,
            port_concurrency: 200,
            max_ports: 2048,
            dns_timeout_ms: 3000,
            whois_timeout_ms: 10_000,
            tls_timeout_ms: 8000,
            http_timeout_ms: 10_000,
            ntp_timeout_ms: 3000,
            resolve_names: true,
        }
    }
}

/// The leash on a sweep, which sends to machines nobody named.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SweepSettings {
    pub max_hosts: u32,
    pub concurrency: usize,
    pub timeout_ms: u64,
    /// Whether a sweep may be asked for addresses outside private,
    /// link-local, unique-local and loopback space at all.
    pub allow_public: bool,
    /// TCP ports tried on an address that does not answer an echo request.
    pub tcp_fallback_ports: String,
}

impl Default for SweepSettings {
    fn default() -> Self {
        SweepSettings {
            max_hosts: 4096,
            concurrency: 128,
            timeout_ms: 800,
            allow_public: false,
            tcp_fallback_ports: "22,23,80,443,8291".into(),
        }
    }
}

/// Where the TFTP server listens and what it allows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TftpSettings {
    /// Address to bind. `0.0.0.0` serves every interface of this machine.
    pub bind: String,
    pub port: u16,
    /// The directory served when a request does not name one.
    pub root: String,
    /// The largest upload accepted, in mebibytes.
    pub max_file_mb: u64,
}

impl Default for TftpSettings {
    fn default() -> Self {
        TftpSettings {
            bind: "0.0.0.0".into(),
            port: 6969,
            root: String::new(),
            max_file_mb: 512,
        }
    }
}

/// Where the staging syslog listener listens and how much it keeps.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyslogSettings {
    pub bind: String,
    pub port: u16,
    /// Also listen on TCP (RFC 6587), for devices configured that way.
    pub tcp: bool,
    /// Messages kept in memory for `probe.syslog.messages`.
    pub keep_messages: u32,
}

impl Default for SyslogSettings {
    fn default() -> Self {
        SyslogSettings {
            bind: "0.0.0.0".into(),
            port: 5515,
            tcp: false,
            keep_messages: 2000,
        }
    }
}

impl ToolboxSettings {
    /// The settings as stored, with values that would do harm brought into range.
    pub fn load(app: &App) -> ToolboxSettings {
        let stored: ToolboxSettings = nocturne_app::settings::get(app, SECTION).unwrap_or_default();
        let probe = ProbeSettings {
            ping_count: stored.probe.ping_count.clamp(1, 10_000),
            ping_interval_ms: stored.probe.ping_interval_ms.clamp(10, 60_000),
            ping_timeout_ms: stored.probe.ping_timeout_ms.clamp(10, 60_000),
            ping_payload: stored.probe.ping_payload.min(65_000),
            max_run_s: stored.probe.max_run_s.clamp(1, MAX_RUN_S),
            trace_max_hops: stored.probe.trace_max_hops.clamp(1, 64),
            trace_probes: stored.probe.trace_probes.clamp(1, 10),
            trace_timeout_ms: stored.probe.trace_timeout_ms.clamp(100, 60_000),
            port_timeout_ms: stored.probe.port_timeout_ms.clamp(10, 60_000),
            port_concurrency: stored.probe.port_concurrency.clamp(1, 1024),
            max_ports: stored.probe.max_ports.clamp(1, MAX_PORTS),
            dns_timeout_ms: stored.probe.dns_timeout_ms.clamp(100, 60_000),
            whois_timeout_ms: stored.probe.whois_timeout_ms.clamp(100, 120_000),
            tls_timeout_ms: stored.probe.tls_timeout_ms.clamp(100, 60_000),
            http_timeout_ms: stored.probe.http_timeout_ms.clamp(100, 120_000),
            ntp_timeout_ms: stored.probe.ntp_timeout_ms.clamp(100, 60_000),
            ..stored.probe
        };
        let sweep = SweepSettings {
            max_hosts: stored.sweep.max_hosts.clamp(1, MAX_SWEEP_HOSTS),
            concurrency: stored.sweep.concurrency.clamp(1, 512),
            timeout_ms: stored.sweep.timeout_ms.clamp(50, 30_000),
            ..stored.sweep
        };
        let tftp = TftpSettings {
            max_file_mb: stored.tftp.max_file_mb.clamp(1, 16_384),
            ..stored.tftp
        };
        let syslog = SyslogSettings {
            keep_messages: stored.syslog.keep_messages.clamp(10, 100_000),
            ..stored.syslog
        };
        ToolboxSettings {
            probe,
            sweep,
            tftp,
            syslog,
            keep_runs: stored.keep_runs.clamp(1, 2000),
        }
    }
}

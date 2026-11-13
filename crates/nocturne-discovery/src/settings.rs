//! The `discovery` settings section: the bounds on what a sweep may touch.
//!
//! A sweep sends packets to machines nobody asked Nocturne to talk to, so the
//! numbers here are not preferences, they are the leash. Every one of them is
//! clamped on read: a typo in the settings screen cannot open ten thousand
//! sockets, and a scan request can only ask for *less* than the settings
//! allow, never more.

use nocturne_app::App;
use serde::{Deserialize, Serialize};

pub const SECTION: &str = "discovery";

/// Hard ceiling on `max_hosts`, whatever the settings say.
pub const MAX_HOSTS_LIMIT: u32 = 65_536;
/// Hard ceiling on the probe rate.
pub const RATE_LIMIT: u32 = 20_000;
/// Hard ceiling on concurrency.
pub const CONCURRENCY_LIMIT: u32 = 512;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscoverySettings {
    /// The most addresses one scan may cover.
    pub max_hosts: u32,
    /// Probes per second across a whole scan (echo, TCP connect, SNMP get).
    pub rate: u32,
    /// Addresses probed at the same time.
    pub concurrency: u32,
    /// How long one probe waits for an answer.
    pub timeout_ms: u64,
    /// TCP ports tried on each address when a scan does not name its own.
    pub ports: String,
    /// Whether a scan may ask for addresses outside private, link-local and
    /// loopback space at all. Off: `allow_public` on a request is refused.
    pub allow_public: bool,
    /// Address blocks that are never probed, whatever a request says
    /// (`10.9.0.0/16, 192.0.2.7`). One per entry or comma-separated.
    pub never_scan: Vec<String>,
    /// How many finished scans are kept.
    pub keep_scans: u32,
    /// The deepest a neighbour crawl may go from its seeds.
    pub max_crawl_depth: u32,
}

impl Default for DiscoverySettings {
    fn default() -> Self {
        DiscoverySettings {
            max_hosts: 4_096,
            rate: 500,
            concurrency: 64,
            timeout_ms: 800,
            ports: "22,23,80,443,161,8291".into(),
            allow_public: false,
            never_scan: Vec::new(),
            keep_scans: 20,
            max_crawl_depth: 3,
        }
    }
}

impl DiscoverySettings {
    /// The settings as stored, with values that would do harm brought into range.
    pub fn load(app: &App) -> DiscoverySettings {
        let stored: DiscoverySettings =
            nocturne_app::settings::get(app, SECTION).unwrap_or_default();
        DiscoverySettings {
            max_hosts: stored.max_hosts.clamp(1, MAX_HOSTS_LIMIT),
            rate: stored.rate.clamp(1, RATE_LIMIT),
            concurrency: stored.concurrency.clamp(1, CONCURRENCY_LIMIT),
            timeout_ms: stored.timeout_ms.clamp(50, 30_000),
            keep_scans: stored.keep_scans.clamp(1, 200),
            max_crawl_depth: stored.max_crawl_depth.clamp(1, 8),
            ..stored
        }
    }
}

/// What one scan is allowed to do: the request, narrowed by the settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_hosts: u32,
    pub rate: u32,
    pub concurrency: u32,
    pub timeout_ms: u64,
}

impl DiscoverySettings {
    /// A request may lower any of these; asking for more than the settings
    /// allow silently gets the settings' value, because the settings are the
    /// administrator's decision and the request is a user's wish.
    pub fn limits(&self, asked: &crate::model::ScanRequest) -> Limits {
        let narrow = |asked: u32, allowed: u32| match asked {
            0 => allowed,
            asked => asked.min(allowed),
        };
        Limits {
            max_hosts: narrow(asked.max_hosts, self.max_hosts),
            rate: narrow(asked.rate, self.rate),
            concurrency: narrow(asked.concurrency, self.concurrency),
            timeout_ms: match asked.timeout_ms {
                0 => self.timeout_ms,
                asked => asked.clamp(50, 30_000),
            },
        }
    }
}

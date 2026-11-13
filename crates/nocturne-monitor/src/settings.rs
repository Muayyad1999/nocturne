//! The `monitoring` settings section.

use nocturne_app::App;
use serde::{Deserialize, Serialize};

pub const SECTION: &str = "monitoring";

/// Echo requests per reachability probe.
pub const PING_COUNT: u32 = 3;
/// How often a lab device's CLI is read, in seconds (they are cheap and local).
pub const LAB_CLI_INTERVAL_S: u32 = 30;
/// Per-attempt SNMP timeout, in seconds.
pub const SNMP_TIMEOUT_S: u64 = 2;
/// Interface rows one SNMP poll will read.
pub const SNMP_TABLE_LIMIT: usize = 20_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MonitorSettings {
    /// Turns the scheduler off without touching any device's own setting.
    pub enabled: bool,
    /// Probes in flight across the whole network.
    pub concurrency: u32,
    /// Consecutive failed polls before a device is called down.
    pub down_after: u32,
    /// How often devices without SNMP are read over the CLI.
    pub cli_interval_s: u32,
    pub retention_days_raw: u32,
    pub retention_days_rollup: u32,
    pub ping_timeout_ms: u64,
}

impl Default for MonitorSettings {
    fn default() -> Self {
        MonitorSettings {
            enabled: true,
            concurrency: 32,
            down_after: 2,
            cli_interval_s: 300,
            retention_days_raw: 7,
            retention_days_rollup: 365,
            ping_timeout_ms: 1000,
        }
    }
}

impl MonitorSettings {
    /// The settings as stored, with values that would do harm brought into
    /// range: a typo cannot open ten thousand sockets at once, and damping
    /// of zero would make every lost packet an outage.
    pub fn load(app: &App) -> MonitorSettings {
        let stored: MonitorSettings = nocturne_app::settings::get(app, SECTION).unwrap_or_default();
        MonitorSettings {
            concurrency: stored.concurrency.clamp(1, 512),
            down_after: stored.down_after.clamp(1, 100),
            cli_interval_s: stored.cli_interval_s.clamp(30, 86_400),
            retention_days_raw: stored.retention_days_raw.clamp(1, 3_650),
            retention_days_rollup: stored.retention_days_rollup.clamp(1, 3_650),
            ping_timeout_ms: stored.ping_timeout_ms.clamp(50, 30_000),
            ..stored
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn settings_are_declared_with_the_documented_defaults_and_clamped() {
        let mut builder = nocturne_app::testing::builder();
        crate::init(&mut builder);
        let app = builder.build();

        let stored = app
            .rpc("settings.get", json!({"section": SECTION}))
            .await
            .unwrap();
        assert_eq!(
            stored,
            json!({"enabled": true, "concurrency": 32, "down_after": 2, "cli_interval_s": 300,
                   "retention_days_raw": 7, "retention_days_rollup": 365, "ping_timeout_ms": 1000})
        );

        app.rpc(
            "settings.set",
            json!({"section": SECTION, "value": {"concurrency": 0, "down_after": 0, "cli_interval_s": 1}}),
        )
        .await
        .unwrap();
        let settings = MonitorSettings::load(&app);
        assert_eq!(
            (
                settings.concurrency,
                settings.down_after,
                settings.cli_interval_s
            ),
            (1, 1, 30)
        );
    }
}

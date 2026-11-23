//! The `monitoring` settings section.

use nocturne_app::App;
use serde::{Deserialize, Serialize};

pub const SECTION: &str = "monitoring";

/// Echo requests per reachability probe.
pub const PING_COUNT: u32 = 3;
/// How often a lab device's CLI is read, in seconds (they are cheap and local).
pub const LAB_CLI_INTERVAL_S: u32 = 30;
/// How often a lab device's QoS statistics are read, in seconds.
pub const LAB_QOS_INTERVAL_S: u32 = 30;
/// How long a command-line session that is only for QoS waits for another
/// automated session on the device (a change, a backup) to finish, in
/// seconds. The poll it sits in has usually read the device over SNMP
/// already, and waits with it.
pub const QOS_SESSION_WAIT_S: u64 = 5;
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
    /// Turns the service checks off without touching any check's own
    /// setting, the way `enabled` does for the device poller.
    pub checks_enabled: bool,
    /// How long a service check's changes of state are kept.
    pub check_history_days: u32,
    /// Days of certificate life left at which a TLS or HTTPS check goes to
    /// `warning`. A check may set its own.
    pub cert_warn_days: u32,
    /// Whether an alert about a lab device is announced at all. Off by
    /// default: a lab is where things are broken on purpose, and its alerts
    /// are recorded and shown without waking anybody.
    pub notify_for_lab: bool,
    /// How often a device's per-class QoS statistics are read over its
    /// command line. Its own cadence, because the read is a CLI session
    /// also on a device whose interfaces are polled over SNMP.
    pub qos_interval_s: u32,
    /// The most classes of one device that get time series. A router with a
    /// queue per subscriber has thousands; the current table holds them all
    /// and `monitor.qos` says when the series were cut here.
    pub qos_max_series: u32,
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
            checks_enabled: true,
            check_history_days: 90,
            cert_warn_days: 14,
            notify_for_lab: false,
            qos_interval_s: 300,
            qos_max_series: 256,
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
            check_history_days: stored.check_history_days.clamp(1, 3_650),
            // Zero would mean "warn about every certificate", which is not a
            // warning; a check that wants no warning says so itself.
            cert_warn_days: stored.cert_warn_days.clamp(1, 365),
            qos_interval_s: stored.qos_interval_s.clamp(30, 86_400),
            // Zero is allowed and means what it says: a table, no series.
            // The ceiling is five series a class, per device, every read.
            qos_max_series: stored.qos_max_series.min(10_000),
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
                   "retention_days_raw": 7, "retention_days_rollup": 365, "ping_timeout_ms": 1000,
                   "checks_enabled": true, "check_history_days": 90, "cert_warn_days": 14,
                   "notify_for_lab": false, "qos_interval_s": 300, "qos_max_series": 256})
        );

        app.rpc(
            "settings.set",
            json!({"section": SECTION, "value": {"concurrency": 0, "down_after": 0, "cli_interval_s": 1,
                                                 "cert_warn_days": 0, "check_history_days": 0,
                                                 "qos_interval_s": 1, "qos_max_series": 2_000_000}}),
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
        assert_eq!(
            (settings.cert_warn_days, settings.check_history_days),
            (1, 1),
            "a warning of zero days is not a warning"
        );
        assert_eq!(
            (settings.qos_interval_s, settings.qos_max_series),
            (30, 10_000),
            "a CLI session a second and a million series a device are both typos"
        );
    }
}

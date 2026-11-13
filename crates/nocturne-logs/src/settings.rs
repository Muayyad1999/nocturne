//! The `logs` settings section.
//!
//! The receiver is **off by default**: opening a listening socket is the
//! user's decision, not a side effect of starting the application. The
//! default ports are unprivileged (5514, not 514) because binding a port
//! below 1024 needs rights a desktop application does not have, and a
//! receiver that cannot bind says so in `logs.status` instead of dying
//! quietly.

use nocturne_app::App;
use serde::{Deserialize, Serialize};

pub const SECTION: &str = "logs";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogsSettings {
    pub syslog: SyslogSettings,
    /// Syslog entries older than this are pruned. 0 = keep until `max_rows`.
    pub retention_days: u32,
    /// Hard ceiling on stored syslog rows; the oldest go first.
    pub max_rows: u64,
    /// What to do with a line from an address no device in the inventory
    /// claims: `store` (keep it with no device, so the user can see it and
    /// add the device) or `drop`.
    pub unknown_sources: String,
    /// Lines that may wait to be written before the receiver drops and
    /// counts. Each one costs a few hundred bytes, so this is the memory the
    /// receiver may use to absorb a burst; beyond it, dropping and saying so
    /// is better than growing until the machine dies.
    pub queue_limit: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyslogSettings {
    pub enabled: bool,
    pub udp_port: u16,
    /// 0 turns the TCP listener off (UDP only).
    pub tcp_port: u16,
    pub bind: String,
}

impl Default for SyslogSettings {
    fn default() -> Self {
        SyslogSettings {
            enabled: false,
            udp_port: 5514,
            tcp_port: 5514,
            bind: "0.0.0.0".into(),
        }
    }
}

impl Default for LogsSettings {
    fn default() -> Self {
        LogsSettings {
            syslog: SyslogSettings::default(),
            retention_days: 30,
            max_rows: 2_000_000,
            unknown_sources: "store".into(),
            queue_limit: crate::state::DEFAULT_QUEUE_LIMIT as u32,
        }
    }
}

impl LogsSettings {
    /// The settings as stored, with values that would do harm brought into
    /// range: an unparsable `unknown_sources` keeps the line rather than
    /// silently throwing the network's logs away, and a row cap of zero would
    /// mean "store nothing" rather than "no cap".
    pub fn load(app: &App) -> LogsSettings {
        let stored: LogsSettings = nocturne_app::settings::get(app, SECTION).unwrap_or_default();
        LogsSettings {
            retention_days: stored.retention_days.min(3_650),
            max_rows: stored.max_rows.clamp(1_000, 500_000_000),
            unknown_sources: match stored.unknown_sources.trim().to_ascii_lowercase().as_str() {
                "drop" => "drop".into(),
                _ => "store".into(),
            },
            queue_limit: stored.queue_limit.clamp(100, 1_000_000),
            ..stored
        }
    }

    pub fn drop_unknown(&self) -> bool {
        self.unknown_sources == "drop"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn the_receiver_is_off_until_someone_turns_it_on() {
        let mut builder = nocturne_app::testing::builder();
        crate::init(&mut builder);
        let app = builder.build();

        let stored = app
            .rpc("settings.get", json!({"section": SECTION}))
            .await
            .unwrap();
        assert_eq!(
            stored,
            json!({
                "syslog": {"enabled": false, "udp_port": 5514, "tcp_port": 5514, "bind": "0.0.0.0"},
                "retention_days": 30,
                "max_rows": 2_000_000,
                "unknown_sources": "store",
                "queue_limit": 50_000,
            })
        );
    }

    #[tokio::test]
    async fn nonsense_settings_are_brought_into_range() {
        let mut builder = nocturne_app::testing::builder();
        crate::init(&mut builder);
        let app = builder.build();
        app.rpc(
            "settings.set",
            json!({"section": SECTION, "value": {"max_rows": 0, "unknown_sources": "sometimes"}}),
        )
        .await
        .unwrap();
        let settings = LogsSettings::load(&app);
        assert_eq!(settings.max_rows, 1_000);
        assert!(
            !settings.drop_unknown(),
            "an unrecognised value keeps the lines rather than discarding them"
        );
    }
}

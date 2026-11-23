//! The `logs` settings section.
//!
//! Every receiver is **off by default**: opening a listening socket is the
//! user's decision, not a side effect of starting the application. The
//! default ports are unprivileged (5514 and 1162, not 514 and 162; flow
//! export has no privileged port, and 2055 is where exporters send by
//! custom) because binding a port below 1024 needs rights a desktop
//! application does not have, and a receiver that cannot bind says so in
//! `logs.status` instead of dying quietly.
//!
//! Nothing here is a secret, and nothing here may become one: the trap
//! receiver checks a community against the one in the vault, so there is
//! no list of accepted communities to keep in a document that
//! `settings.get` returns.

use nocturne_app::App;
use serde::{Deserialize, Serialize};

pub const SECTION: &str = "logs";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogsSettings {
    pub syslog: SyslogSettings,
    pub traps: TrapSettings,
    pub flows: FlowSettings,
    /// Syslog lines, traps and audit entries older than this are pruned.
    /// 0 = keep until `max_rows`.
    pub retention_days: u32,
    /// Hard ceiling on stored syslog lines and traps together; the oldest
    /// go first.
    pub max_rows: u64,
    /// What to do with a line from an address no device in the inventory
    /// claims: `store` (keep it with no device, so the user can see it and
    /// add the device) or `drop`. For the flow collector: count what such
    /// an exporter sends, or list the exporter and count nothing of it.
    pub unknown_sources: String,
    /// Lines that may wait to be written before a receiver drops and
    /// counts (each receiver has its own queue of this size; for the flow
    /// collector it is flow records waiting to be counted). Each one costs
    /// a few hundred bytes, so this is the memory a receiver may use to
    /// absorb a burst; beyond it, dropping and saying so is better than
    /// growing until the machine dies.
    pub queue_limit: u32,
}

/// The flow collector: NetFlow v5, NetFlow v9 and IPFIX on one UDP port.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FlowSettings {
    pub enabled: bool,
    pub port: u16,
    pub bind: String,
}

impl Default for FlowSettings {
    fn default() -> Self {
        FlowSettings {
            enabled: false,
            port: 2055,
            bind: "0.0.0.0".into(),
        }
    }
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

/// The SNMP trap receiver.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrapSettings {
    pub enabled: bool,
    pub port: u16,
    pub bind: String,
    /// Keep an SNMPv1/v2c notification only when its community is the one
    /// stored for the device it came from. Off: any community is taken from
    /// a device in the inventory, and an unknown source follows
    /// `unknown_sources`.
    pub require_known_community: bool,
}

impl Default for TrapSettings {
    fn default() -> Self {
        TrapSettings {
            enabled: false,
            port: 1162,
            bind: "0.0.0.0".into(),
            require_known_community: true,
        }
    }
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
            traps: TrapSettings::default(),
            flows: FlowSettings::default(),
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
    async fn every_receiver_is_off_until_someone_turns_it_on() {
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
                "traps": {"enabled": false, "port": 1162, "bind": "0.0.0.0", "require_known_community": true},
                "flows": {"enabled": false, "port": 2055, "bind": "0.0.0.0"},
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

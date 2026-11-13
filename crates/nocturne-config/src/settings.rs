//! The `config` settings section.

use nocturne_app::App;
use serde::{Deserialize, Serialize};

pub const SECTION: &str = "config";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigSettings {
    /// Versions kept per device and kind; older ones are pruned unless pinned.
    pub keep_versions: u32,
    /// Fetch the startup configuration too, so unsaved changes can be told.
    pub backup_startup: bool,
    /// Back a device up when the log feature reports that its configuration changed.
    pub backup_on_change_event: bool,
    /// Devices backed up at the same time.
    pub concurrency: u32,
}

impl Default for ConfigSettings {
    fn default() -> Self {
        ConfigSettings {
            keep_versions: 200,
            backup_startup: true,
            backup_on_change_event: true,
            concurrency: 6,
        }
    }
}

impl ConfigSettings {
    /// The settings as stored, with values that would do harm brought into
    /// range: retention never drops the newest version, and a typo cannot
    /// open hundreds of sessions at once.
    pub fn load(app: &App) -> ConfigSettings {
        let stored: ConfigSettings = nocturne_app::settings::get(app, SECTION).unwrap_or_default();
        ConfigSettings {
            keep_versions: stored.keep_versions.max(1),
            concurrency: stored.concurrency.clamp(1, 32),
            ..stored
        }
    }
}

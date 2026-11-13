//! The `ops` settings section.

use serde::{Deserialize, Serialize};

use nocturne_app::{App, AppBuilder, AppResult};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// How long an overview may be reused before it is made again. The home
    /// screen asks for one every 30 s; several panes asking at once should
    /// not re-read the estate several times.
    pub overview_cache_ms: u64,
    /// How long `search.query` waits for one of the features it fans out to.
    pub search_budget_ms: u64,
    /// How far before an incident started `incidents.suggest_links` looks for
    /// something that might have caused it. A configuration change four
    /// minutes before an outage is the point of the whole method.
    pub suggest_lead_minutes: u32,
    /// How far a configuration change may sit outside an applied change's own
    /// window and still count as that change's work. Beyond it, the handover
    /// calls the configuration change `unplanned`.
    pub unplanned_grace_minutes: u32,
    /// How many devices an availability report measures. Each device is a
    /// separate query over the sample store, so a report over a year of a
    /// large estate is deliberately refused rather than run for minutes.
    pub availability_max_devices: usize,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            overview_cache_ms: 5_000,
            search_budget_ms: 300,
            suggest_lead_minutes: 30,
            unplanned_grace_minutes: 10,
            availability_max_devices: 2_000,
        }
    }
}

pub const SECTION: &str = "ops";

pub fn declare(builder: &mut AppBuilder) {
    nocturne_app::settings::declare(builder, SECTION, &Settings::default());
}

pub fn get(app: &App) -> AppResult<Settings> {
    nocturne_app::settings::get(app, SECTION)
}

//! The `detect` settings section.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use nocturne_app::{App, AppBuilder};

pub const SECTION: &str = "detect";

/// "This many in that long", for the detections that count.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Threshold {
    pub count: u32,
    pub window_min: u32,
}

impl Threshold {
    pub fn window_ms(self) -> i64 {
        i64::from(self.window_min) * 60_000
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Off silences every detection without forgetting which ones were on.
    pub enabled: bool,
    /// How far either side of an event an approved change still explains it.
    /// The time of a syslog line is the time it was received, so the
    /// comparison needs this slack.
    pub grace_minutes: u32,
    pub unredacted_views: Threshold,
    pub failed_unlocks: Threshold,
    /// Catalogue ids switched off one by one.
    pub disabled: Vec<String>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            enabled: true,
            grace_minutes: 15,
            unredacted_views: Threshold {
                count: 5,
                window_min: 10,
            },
            failed_unlocks: Threshold {
                count: 5,
                window_min: 5,
            },
            disabled: Vec::new(),
        }
    }
}

/// A whole number from a stored setting, brought into `low..=high`; `None`
/// when what is stored is not a number at all.
fn number(value: &Value, low: u32, high: u32) -> Option<u32> {
    let number = value
        .as_i64()
        .or_else(|| value.as_u64().map(|_| i64::MAX))
        .or_else(|| value.as_f64().map(|number| number as i64))?;
    Some(number.clamp(i64::from(low), i64::from(high)) as u32)
}

impl Settings {
    /// The settings as stored, with values that would make a detection
    /// meaningless brought into range: a threshold of zero would fire on
    /// nothing at all, and a window of zero could never be met.
    ///
    /// Read key by key. `settings.set` stores whatever it is handed, and if
    /// one value of the wrong type made the whole section unreadable, a
    /// mistyped number would silently switch back on everything that had
    /// been switched off: a key that cannot be read takes its own default,
    /// and the others stay as they are.
    pub fn load(app: &App) -> Settings {
        let defaults = Settings::default();
        let stored = match nocturne_app::settings::get_value(app, SECTION) {
            Ok(stored) => stored,
            Err(err) => {
                tracing::error!(%err, "the detect settings cannot be read; using the defaults");
                return defaults;
            }
        };
        let threshold = |key: &str, default: Threshold| Threshold {
            count: number(&stored[key]["count"], 1, u32::MAX).unwrap_or(default.count),
            window_min: number(&stored[key]["window_min"], 1, 24 * 60)
                .unwrap_or(default.window_min),
        };
        Settings {
            enabled: stored["enabled"].as_bool().unwrap_or(defaults.enabled),
            grace_minutes: number(&stored["grace_minutes"], 0, 24 * 60)
                .unwrap_or(defaults.grace_minutes),
            unredacted_views: threshold("unredacted_views", defaults.unredacted_views),
            failed_unlocks: threshold("failed_unlocks", defaults.failed_unlocks),
            disabled: stored["disabled"]
                .as_array()
                .map(|ids| {
                    ids.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or(defaults.disabled),
        }
    }

    pub fn grace_ms(&self) -> i64 {
        i64::from(self.grace_minutes) * 60_000
    }

    /// Whether a detection is switched on (which says nothing about whether
    /// this build can run it).
    pub fn is_on(&self, id: &str) -> bool {
        self.enabled && !self.disabled.iter().any(|disabled| disabled == id)
    }
}

pub fn declare(builder: &mut AppBuilder) {
    nocturne_app::settings::declare(builder, SECTION, &Settings::default());
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_stored_number_is_brought_into_range_and_what_is_no_number_is_not_one() {
        assert_eq!(number(&json!(15), 0, 1_440), Some(15));
        assert_eq!(number(&json!(-5), 0, 1_440), Some(0));
        assert_eq!(number(&json!(0), 1, 1_440), Some(1));
        assert_eq!(number(&json!(9_000_000), 1, 1_440), Some(1_440));
        assert_eq!(number(&json!(u64::MAX), 1, 1_440), Some(1_440));
        assert_eq!(number(&json!(7.9), 1, 1_440), Some(7));
        assert_eq!(number(&json!("15"), 0, 1_440), None);
        assert_eq!(number(&json!(null), 0, 1_440), None);
    }
}

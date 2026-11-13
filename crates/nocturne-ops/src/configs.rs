//! Configuration movement, read from the core's own snapshot table.
//!
//! The configuration feature stores every backup through
//! `store.config_insert`, which keeps a version only when the content is
//! different from the one before it. A version is therefore a configuration
//! that really moved, and the core store is the record of it — which is why
//! this is read from the store rather than asked of the configuration
//! feature: it is as true in a build without that feature as in one with it.

use serde::{Deserialize, Serialize};

use nocturne_app::AppResult;
use nocturne_app::store::Store;

use crate::refs::Period;

/// A moment at which a device's running configuration changed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConfigChange {
    pub device_id: String,
    /// The stored version that holds the new content.
    pub version_id: i64,
    /// The version it replaced; absent for a device's first backup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_version_id: Option<i64>,
    pub at: i64,
    /// `None` when the diff was not computed (the first backup, or the
    /// snapshot could not be read).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines_added: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines_removed: Option<usize>,
    /// How many lines the new version has.
    pub lines: u32,
    /// Who took the backup: `manual`, `scheduled`, `pre-change`,
    /// `post-change`, `import`.
    pub source: String,
    pub note: String,
    /// The first backup of a device is not a change; it is the baseline.
    pub first: bool,
}

impl ConfigChange {
    /// How the change reads in one line, without a device name.
    pub fn describe(&self) -> String {
        let counts = match (self.lines_added, self.lines_removed) {
            (Some(added), Some(removed)) => format!("+{added} −{removed} lines"),
            _ if self.first => format!("first backup, {} lines", self.lines),
            _ => format!("{} lines, change not measured", self.lines),
        };
        let note = if self.note.trim().is_empty() {
            String::new()
        } else {
            format!(", {}", self.note.trim())
        };
        format!("{counts} ({}{note})", self.source)
    }
}

/// How many versions of one device a single read will diff. A device whose
/// configuration is rewritten every minute would otherwise decompress
/// thousands of snapshots for one report.
pub const MAX_PER_DEVICE: usize = 50;

/// The running-configuration changes of one device inside `period`, newest
/// first. `truncated` says the device had more than [`MAX_PER_DEVICE`].
pub fn changes_of(
    store: &Store,
    device_id: &str,
    period: Period,
) -> AppResult<(Vec<ConfigChange>, bool)> {
    // Newest first, every version of the device: the one before the window
    // is needed to diff the first one inside it.
    let versions = store.config_versions(device_id, Some("running"))?;
    let inside: Vec<usize> = versions
        .iter()
        .enumerate()
        .filter(|(_, version)| period.contains(version.taken_at))
        .map(|(index, _)| index)
        .collect();
    let truncated = inside.len() > MAX_PER_DEVICE;
    let mut changes = Vec::new();
    for index in inside.into_iter().take(MAX_PER_DEVICE) {
        let version = &versions[index];
        // `config_versions` is newest first, so the previous content is the
        // next element.
        let previous = versions.get(index + 1);
        let (mut added, mut removed) = (None, None);
        if let Some(previous) = previous
            && let (Ok((_, old)), Ok((_, new))) =
                (store.config_get(previous.id), store.config_get(version.id))
        {
            let diff = nocturne_drivers::hier::text_diff(&old, &new, Some(0));
            added = Some(diff.added);
            removed = Some(diff.removed);
        }
        changes.push(ConfigChange {
            device_id: device_id.to_string(),
            version_id: version.id,
            previous_version_id: previous.map(|version| version.id),
            at: version.taken_at,
            lines_added: added,
            lines_removed: removed,
            lines: version.lines,
            source: version.source.clone(),
            note: version.note.clone(),
            first: previous.is_none(),
        });
    }
    Ok((changes, truncated))
}

/// The configuration changes of several devices inside `period`, newest
/// first across all of them.
pub fn changes_in(
    store: &Store,
    device_ids: &[String],
    period: Period,
) -> AppResult<(Vec<ConfigChange>, Vec<String>)> {
    let mut all = Vec::new();
    let mut truncated = Vec::new();
    for device_id in device_ids {
        let (changes, cut) = changes_of(store, device_id, period)?;
        if cut {
            truncated.push(device_id.clone());
        }
        all.extend(changes);
    }
    all.sort_by(|a, b| {
        b.at.cmp(&a.at)
            .then_with(|| b.version_id.cmp(&a.version_id))
    });
    Ok((all, truncated))
}

/// Whether a device's newest running configuration is older than `at`.
pub fn newest_backup(store: &Store, device_id: &str) -> AppResult<Option<(i64, u32)>> {
    Ok(store
        .config_latest(device_id, "running")?
        .map(|version| (version.taken_at, version.lines)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn store() -> Store {
        Store::in_memory().unwrap()
    }

    #[test]
    fn a_change_is_the_diff_against_the_version_before_it() {
        let store = store();
        // The core stamps `taken_at` itself, so the versions are inserted and
        // then read back to learn their times.
        store
            .config_insert("d1", "running", "hostname SW1\nvlan 10\n", "scheduled", "")
            .unwrap();
        store
            .config_insert(
                "d1",
                "running",
                "hostname SW1\nvlan 10\nvlan 20\nvlan 30\n",
                "post-change",
                "added vlans",
            )
            .unwrap();
        store
            .config_insert("d1", "running", "hostname SW1\nvlan 20\n", "scheduled", "")
            .unwrap();
        let all = Period {
            since: 0,
            until: i64::MAX,
        };
        let (changes, truncated) = changes_of(&store, "d1", all).unwrap();
        assert!(!truncated);
        assert_eq!(changes.len(), 3, "newest first");
        assert_eq!(
            (changes[0].lines_added, changes[0].lines_removed),
            (Some(0), Some(2)),
            "two vlans gone"
        );
        assert_eq!(
            (changes[1].lines_added, changes[1].lines_removed),
            (Some(2), Some(0))
        );
        assert!(
            changes[1]
                .describe()
                .contains("+2 −0 lines (post-change, added vlans)")
        );
        // The oldest version is the baseline, not a change.
        assert!(changes[2].first);
        assert_eq!(changes[2].lines_added, None);
        assert!(changes[2].describe().contains("first backup, 2 lines"));
        assert_eq!(changes[2].previous_version_id, None);

        // Identical content is not a version, so it is not a change either.
        store
            .config_insert("d1", "running", "hostname SW1\nvlan 20\n", "scheduled", "")
            .unwrap();
        assert_eq!(changes_of(&store, "d1", all).unwrap().0.len(), 3);
        // A startup-config backup is not a running-config change.
        store
            .config_insert("d1", "startup", "hostname SW1\n", "manual", "")
            .unwrap();
        assert_eq!(changes_of(&store, "d1", all).unwrap().0.len(), 3);
        assert_eq!(newest_backup(&store, "d1").unwrap().unwrap().1, 2);
        assert_eq!(newest_backup(&store, "nobody").unwrap(), None);
    }

    #[test]
    fn a_window_keeps_the_version_before_it_for_the_diff() {
        let store = store();
        let (first, _) = store
            .config_insert("d1", "running", "a\n", "scheduled", "")
            .unwrap();
        let (second, _) = store
            .config_insert("d1", "running", "a\nb\n", "scheduled", "")
            .unwrap();
        // A window that starts at the second version still diffs against the
        // first, which is outside it. (The core stamps both with its own
        // clock, so in a test they land in the same millisecond and the
        // window holds both; what matters is the diff.)
        let window = Period {
            since: second.taken_at,
            until: second.taken_at + 1,
        };
        let (changes, _) = changes_of(&store, "d1", window).unwrap();
        assert_eq!(changes[0].version_id, second.id, "newest first");
        assert_eq!(changes[0].previous_version_id, Some(first.id));
        assert_eq!(changes[0].lines_added, Some(1));
        assert!(!changes[0].first);
        // A window before everything is empty, not an error.
        let before = Period {
            since: 0,
            until: first.taken_at,
        };
        assert!(changes_of(&store, "d1", before).unwrap().0.is_empty());
    }

    #[test]
    fn several_devices_are_merged_newest_first() {
        let store = store();
        for device in ["d1", "d2"] {
            store
                .config_insert(device, "running", "a\n", "scheduled", "")
                .unwrap();
        }
        let (changes, truncated) = changes_in(
            &store,
            &["d1".to_string(), "d2".to_string(), "gone".to_string()],
            Period {
                since: 0,
                until: i64::MAX,
            },
        )
        .unwrap();
        assert_eq!(changes.len(), 2);
        assert!(changes[0].at >= changes[1].at);
        assert!(truncated.is_empty());
    }
}

//! What the stored configurations say about how the devices are looked after.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use nocturne_ai::Redactor;
use nocturne_app::model::Transport;
use nocturne_drivers::hier::ConfigTree;
use regex::Regex;

use crate::checks::skip_rest;
use crate::model::{Finding, Severity, Suggestion};
use crate::report::CheckReport;
use crate::settings::Settings;
use crate::world::{DeviceData, World};

/// Lines that differ between two reads of an unchanged device. The platform's
/// own `config.ignore` patterns are applied as well (they are why the
/// configuration feature can tell an unchanged device from a changed one);
/// these are the ones common to IOS-style platforms.
static VOLATILE: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"^!\s*Last configuration change",
        r"^!\s*NVRAM config last updated",
        r"^!\s*No configuration change since last restart",
        r"^Current configuration",
        r"^Building configuration",
        r"^!\s*Time:",
        r"^ntp clock-period",
        // Certificate and key material is re-encoded on write by some images.
        r"^\s*quit$",
    ]
    .iter()
    .filter_map(|pattern| Regex::new(pattern).ok())
    .collect()
});

fn is_volatile(line: &str, extra: &[Regex]) -> bool {
    let trimmed = line.trim();
    trimmed.is_empty()
        || trimmed == "!"
        || trimmed == "end"
        || VOLATILE.iter().any(|pattern| pattern.is_match(trimmed))
        || extra.iter().any(|pattern| pattern.is_match(trimmed))
}

/// A configuration reduced to the lines that are actually configuration,
/// counted — so that a move of a line is not reported as two changes.
fn meaningful(text: &str, data: &DeviceData) -> BTreeMap<String, usize> {
    let tree = ConfigTree::parse(text, data.syntax);
    let mut lines: BTreeMap<String, usize> = BTreeMap::new();
    for line in tree.render().lines() {
        if is_volatile(line, &data.config_ignore) {
            continue;
        }
        *lines.entry(line.trim_end().to_string()).or_insert(0) += 1;
    }
    lines
}

// ----------------------------------------------------------- unsaved config

/// The device would come back from a power cut with a different configuration.
///
/// Look-alikes this stays quiet about:
///
/// * **Volatile lines.** `! Last configuration change at …`, `! NVRAM config
///   last updated …`, `ntp clock-period`, and whatever else the platform's
///   profile lists as display rather than configuration. Those differ between
///   any two reads and are not unsaved work.
/// * **Snapshots from different moments.** If the running configuration was
///   read an hour after the startup one, the device may well have been saved
///   in between; the check says "not evaluated" rather than guessing.
pub fn unsaved_config(world: &World, settings: &Settings, report: &mut CheckReport) {
    let window = i64::from(settings.thresholds.saved_pair_window_minutes) * 60_000;
    let redactor = Redactor::new();
    for data in world.targets() {
        let Some(running) = data.config.as_ref() else {
            report.skip(data.id(), "no running configuration stored");
            continue;
        };
        let Some(startup) = data.startup.as_ref() else {
            report.skip(
                data.id(),
                "no startup configuration stored, so there is nothing to compare the running one with",
            );
            continue;
        };
        if (running.taken_at - startup.taken_at).abs() > window {
            report.skip(
                data.id(),
                "the running and startup snapshots were not captured close enough together to be \
                 compared",
            );
            continue;
        }
        report.ok(data.id());
        let running_lines = meaningful(&running.tree.render(), data);
        let startup_lines = meaningful(&startup.text, data);
        if running_lines == startup_lines {
            continue;
        }
        let mut added: Vec<String> = Vec::new();
        let mut removed: Vec<String> = Vec::new();
        for (line, count) in &running_lines {
            let before = startup_lines.get(line).copied().unwrap_or(0);
            for _ in before..*count {
                added.push(line.clone());
            }
        }
        for (line, count) in &startup_lines {
            let after = running_lines.get(line).copied().unwrap_or(0);
            for _ in after..*count {
                removed.push(line.clone());
            }
        }
        if added.is_empty() && removed.is_empty() {
            continue;
        }
        let sample = |lines: &[String]| -> String {
            let shown: Vec<String> = lines
                .iter()
                .take(5)
                .map(|line| redactor.redact(line.trim()))
                .collect();
            let more = lines.len().saturating_sub(shown.len());
            if more > 0 {
                format!("{} … and {more} more", shown.join(" / "))
            } else {
                shown.join(" / ")
            }
        };
        let mut finding = Finding::new(
            "unsaved_config",
            data.id(),
            "running-config",
            format!(
                "{} has {} unsaved configuration line(s)",
                data.name(),
                added.len() + removed.len()
            ),
        )
        .detail(
            "The running configuration differs from the one the device would load after a \
             restart. Whatever was changed will be silently undone by the next power cut or \
             reload — which is exactly when nobody will remember what it was.",
        )
        .evidence(
            "Snapshots",
            format!(
                "running version {} and startup version {}",
                running.version_id, startup.version_id
            ),
        );
        if !added.is_empty() {
            finding = finding.evidence("Only in running", sample(&added));
        }
        if !removed.is_empty() {
            finding = finding.evidence("Only in startup", sample(&removed));
        }
        report.found(finding.suggest(
            Suggestion::text(
                "Look at the difference before saving it: an unsaved configuration is sometimes a \
                 change somebody is still testing. If it is right, save it; if it is not, reload \
                 is the cheapest rollback you will ever get.",
            )
            .commands(vec![
                "show archive config differences nvram:startup-config system:running-config"
                    .to_string(),
            ]),
        ));
    }
    skip_rest(world, report, "no stored configuration");
}

// --------------------------------------------------------- no recent backup

/// There is no recent copy of the device's configuration.
///
/// Look-alike: a device with **no CLI access at all** (monitored over ICMP or
/// SNMP only) cannot be backed up, and is not reported.
pub fn no_recent_backup(world: &World, settings: &Settings, report: &mut CheckReport) {
    if !world.config_feature {
        skip_rest(
            world,
            report,
            "the configuration feature is not installed, so nothing is taking backups",
        );
        return;
    }
    let age = i64::from(settings.thresholds.backup_age_days) * 86_400_000;
    for data in world.targets() {
        if data.device.transport == Transport::None {
            report.skip(
                data.id(),
                "the device has no CLI access, so there is nothing to back up",
            );
            continue;
        }
        report.ok(data.id());
        let newest = data.config.as_ref().map(|config| config.taken_at);
        let days = |at: i64| (world.now - at) / 86_400_000;
        match newest {
            Some(at) if world.now - at <= age => {}
            // An ageing backup is lower than one that never happened: on a
            // fleet whose schedule has stopped, every device says this.
            Some(at) => report.found(
                Finding::new(
                    "no_recent_backup",
                    data.id(),
                    "running-config",
                    format!(
                        "The newest configuration backup of {} is {} day(s) old",
                        data.name(),
                        days(at)
                    ),
                )
                .severity(Severity::Low)
                .detail(
                    "Without a recent copy there is nothing to compare today's configuration \
                     with, nothing to restore after a hardware failure, and no way to tell what \
                     somebody changed.",
                )
                .evidence("Newest backup", format!("{} day(s) old", days(at)))
                .evidence(
                    "Threshold",
                    format!("{} day(s)", settings.thresholds.backup_age_days),
                )
                .suggest(Suggestion::text(
                    "Check why the schedule is not reaching this device — credentials and \
                     reachability are the usual reasons — rather than taking one backup by hand.",
                )),
            ),
            None => report.found(
                Finding::new(
                    "no_recent_backup",
                    data.id(),
                    "running-config",
                    format!("{} has never been backed up", data.name()),
                )
                .detail(
                    "Nocturne holds no configuration for this device at all, so it cannot tell \
                     you what changed, cannot check it for compliance, and cannot put it back \
                     after a failure.",
                )
                .evidence("Stored versions", "none".to_string())
                .suggest(Suggestion::text(
                    "Back it up once by hand to see what fails: credentials, reachability and the \
                     device type are what usually stop it.",
                )),
            ),
        }
    }
    skip_rest(world, report, "no device to check");
}

// ------------------------------------------- changed outside the pipeline

/// The configuration changed, and no Nocturne change accounts for it.
///
/// Look-alikes this stays quiet about:
///
/// * **Changes Nocturne made.** Any applied change for the device within the
///   window, and the snapshots the pipeline takes itself (`pre-change`,
///   `post-change`), account for the difference.
/// * **Old news.** A change older than the backup-age threshold has been
///   there long enough to have been seen; it is not re-reported for ever.
pub fn config_changed_outside_nocturne(
    world: &World,
    settings: &Settings,
    report: &mut CheckReport,
) {
    let Some(applied) = world.applied_changes.as_ref() else {
        skip_rest(
            world,
            report,
            "the changes feature is not installed, so Nocturne's own changes cannot be ruled out",
        );
        return;
    };
    let window = i64::from(settings.thresholds.change_window_minutes) * 60_000;
    let recent = i64::from(settings.thresholds.backup_age_days) * 86_400_000;

    for data in world.targets() {
        let versions = match world.store().config_versions(data.id(), Some("running")) {
            Ok(versions) => versions,
            Err(_) => {
                report.skip(
                    data.id(),
                    "the stored configuration versions could not be read",
                );
                continue;
            }
        };
        if versions.len() < 2 {
            report.skip(
                data.id(),
                "fewer than two stored configurations, so no change can be seen",
            );
            continue;
        }
        report.ok(data.id());
        let newest = &versions[0];
        let previous = &versions[1];
        if world.now - newest.taken_at > recent {
            continue;
        }
        if matches!(newest.source.as_str(), "pre-change" | "post-change") {
            continue;
        }
        let covered = applied
            .get(data.id())
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .find(|change| (change.at - newest.taken_at).abs() <= window);
        if covered.is_some() {
            continue;
        }
        report.found(
            Finding::new(
                "config_changed_outside_nocturne",
                data.id(),
                format!("version {}", newest.id),
                format!(
                    "{}'s configuration changed and no Nocturne change explains it",
                    data.name()
                ),
            )
            .detail(
                "Somebody configured this device directly. That is not forbidden, but it means \
                 the change was not planned, not reviewed and cannot be rolled back — and the \
                 next person to look will not know it happened.",
            )
            .evidence(
                "Changed at",
                format!("{} (version {})", newest.taken_at, newest.id),
            )
            .evidence(
                "Previous version",
                format!("{} (version {})", previous.taken_at, previous.id),
            )
            .evidence("How it was captured", newest.source.clone())
            .evidence(
                "Window searched for a Nocturne change",
                format!("± {} minutes", settings.thresholds.change_window_minutes),
            )
            .suggest(
                Suggestion::text(
                    "Read the difference between the two stored versions and decide whether it \
                     should stay. If it should, record it; if it should not, the previous version \
                     is still there to restore.",
                )
                .commands(vec!["show archive config differences".to_string()]),
            ),
        );
    }
    skip_rest(world, report, "no stored configuration versions");
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;

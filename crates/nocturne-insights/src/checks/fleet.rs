//! The estate as a whole, and the devices themselves.

use std::collections::BTreeMap;

use crate::checks::skip_rest;
use crate::metrics;
use crate::model::{Finding, Suggestion};
use crate::report::CheckReport;
use crate::settings::Settings;
use crate::world::{DeviceData, World};

// ------------------------------------------------------- software versions

/// Devices of one model that are not on one software version.
///
/// Look-alike: **a group too small to have a majority.** With two devices on
/// two versions there is no way to say which is the odd one out, so the check
/// does not run until the group reaches the threshold (three by default).
pub fn software_inconsistent(world: &World, settings: &Settings, report: &mut CheckReport) {
    let minimum = settings.thresholds.software_min_group.max(3) as usize;
    // model → version → devices
    let mut models: BTreeMap<String, BTreeMap<String, Vec<&DeviceData>>> = BTreeMap::new();
    for data in &world.devices {
        let Some(facts) = data.facts.as_ref().and_then(|facts| facts.one()) else {
            continue;
        };
        if facts.model.trim().is_empty() || facts.os_version.trim().is_empty() {
            continue;
        }
        models
            .entry(facts.model.trim().to_string())
            .or_default()
            .entry(facts.os_version.trim().to_string())
            .or_default()
            .push(data);
    }

    let group_of = |data: &DeviceData| -> Option<usize> {
        let facts = data.facts.as_ref().and_then(|facts| facts.one())?;
        models
            .get(facts.model.trim())
            .map(|versions| versions.values().map(Vec::len).sum())
    };
    for data in world.targets() {
        match group_of(data) {
            Some(size) if size >= minimum => report.ok(data.id()),
            Some(size) => report.skip(
                data.id(),
                if size <= 1 {
                    "no other device of this model to compare with"
                } else {
                    "too few devices of this model for a majority version"
                },
            ),
            None => report.skip(data.id(), "no model or software version collected"),
        }
    }

    for (model, versions) in &models {
        let total: usize = versions.values().map(Vec::len).sum();
        if total < minimum || versions.len() < 2 {
            continue;
        }
        // The majority; on a tie the alphabetically first version, so that the
        // same devices are reported in every scan.
        let Some((majority, majority_devices)) =
            versions.iter().max_by_key(|(version, devices)| {
                (devices.len(), std::cmp::Reverse((*version).clone()))
            })
        else {
            continue;
        };
        for (version, devices) in versions {
            if version == majority {
                continue;
            }
            for data in devices {
                if !world.is_target(data.id()) {
                    continue;
                }
                let mut finding = Finding::new(
                    "software_inconsistent",
                    data.id(),
                    version.clone(),
                    format!(
                        "{} runs {version} while {} of {} other {model} run {majority}",
                        data.name(),
                        majority_devices.len(),
                        total - devices.len(),
                    ),
                )
                .detail(
                    "Devices of one model on different software have different bugs, different \
                     defaults and sometimes different syntax — so a change that is tested on one \
                     is not tested on the others, and a fault on one cannot be reasoned about \
                     from the others.",
                )
                .evidence("This device", format!("{model} running {version}"))
                .evidence(
                    "Majority",
                    format!("{} device(s) running {majority}", majority_devices.len()),
                )
                .evidence("Group", format!("{total} device(s) of model {model}"))
                .suggest(Suggestion::text(format!(
                    "Bring it to {majority} at the next window, or find out why it was left \
                     behind — a device that cannot take the standard image is usually out of \
                     memory or out of support.",
                )));
                for other in majority_devices.iter().take(5) {
                    finding = finding.related(other.id(), other.name(), majority.clone());
                }
                report.found(finding);
            }
        }
    }
}

// --------------------------------------------------------------- uptime

/// The device restarted between two collections.
///
/// Look-alikes this stays quiet about:
///
/// * **A counter wrap.** `sysUpTime` wraps after 497 days; uptime that went
///   backwards from a value near that is not a reboot.
/// * **Uptime that went backwards by more than the time that passed.** After
///   a restart the new uptime cannot be longer than the gap between the two
///   collections. Anything else is a parsing or source change (the CLI and
///   SNMP disagreeing, a stack member answering), and is not evaluated.
pub fn uptime_reset(world: &World, _settings: &Settings, report: &mut CheckReport) {
    /// 2^32 hundredths of a second, the point at which `sysUpTime` wraps.
    const WRAP_S: u64 = 497 * 86_400;
    for data in world.targets() {
        let (Some(now), Some(before)) = (data.facts.as_ref(), data.previous_facts.as_ref()) else {
            report.skip(data.id(), "only one collection of facts");
            continue;
        };
        let (Some(now_facts), Some(before_facts)) = (now.one(), before.one()) else {
            report.skip(data.id(), "no facts in one of the collections");
            continue;
        };
        let (Some(uptime), Some(was)) = (now_facts.uptime_s, before_facts.uptime_s) else {
            report.skip(
                data.id(),
                "uptime was not collected in one of the collections",
            );
            continue;
        };
        if was >= WRAP_S - 86_400 {
            report.skip(
                data.id(),
                "the previous uptime was close to the point where the counter wraps, so a \
                 smaller value cannot be read as a restart",
            );
            continue;
        }
        if uptime >= was {
            // Uptime going up is a verdict: the device did not restart.
            report.ok(data.id());
            continue;
        }
        let elapsed_s = ((now.at - before.at).max(0) / 1000) as u64;
        // After a restart between the two collections the new uptime is at
        // most the gap between them (plus slack for collection time).
        if uptime > elapsed_s + 600 {
            report.skip(
                data.id(),
                "uptime went backwards by more than the time between the two collections, which \
                 is a changed source rather than a restart",
            );
            continue;
        }
        report.ok(data.id());
        let reason = now_facts
            .extra
            .iter()
            .find(|(key, _)| {
                let key = key.to_ascii_lowercase();
                key.contains("reload") || key.contains("restart") || key.contains("reason")
            })
            .map(|(key, value)| format!("{key}: {value}"));
        let mut finding = Finding::new(
            "uptime_reset",
            data.id(),
            "uptime",
            format!(
                "{} restarted: uptime went from {} to {}",
                data.name(),
                duration(was),
                duration(uptime)
            ),
        )
        .detail(
            "The device has been up for less time than it had been at the previous collection, so \
             it restarted in between. A restart nobody planned takes the unsaved part of the \
             configuration with it, and a device that restarts repeatedly is usually power or \
             heat.",
        )
        .evidence("Uptime now", duration(uptime))
        .evidence("At the previous collection", duration(was))
        .evidence(
            "Between collections",
            format!("{} elapsed", duration(elapsed_s)),
        );
        if let Some(reason) = reason {
            finding = finding.evidence("Reported reason", reason);
        }
        report.found(finding.suggest(
            Suggestion::text(
                "Find out why before anything else: the device's own log, the reload reason and \
                 the power to the cabinet. Check whether the running configuration survived — an \
                 unsaved change will not have.",
            )
            .commands(vec![
                "show version | include uptime|restarted".to_string(),
                "show logging | include RESTART|RELOAD".to_string(),
            ]),
        ));
    }
    skip_rest(world, report, "no facts collected");
}

fn duration(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

// ----------------------------------------------------- sustained resources

/// One sustained-metric check: CPU, memory and temperature differ only in the
/// metric, the threshold and the words.
fn sustained(
    world: &World,
    settings: &Settings,
    report: &mut CheckReport,
    check: &'static str,
    metric: &str,
    threshold: f64,
    unit: &str,
    what: &str,
    detail: &str,
) {
    let thresholds = &settings.thresholds;
    let window = i64::from(thresholds.sustained_minutes.max(15)) * 4 * 60_000;
    let since = world.now - window;
    for data in world.targets() {
        if !world.has_metric(data.id(), metric) {
            report.skip(data.id(), "this metric is not polled for the device");
            continue;
        }
        let Ok(summary) = metrics::summarise(world.store(), data.id(), metric, since, threshold)
        else {
            report.skip(data.id(), "the samples of this metric could not be read");
            continue;
        };
        if summary.is_empty() {
            report.skip(data.id(), "no samples of this metric inside the window");
            continue;
        }
        report.ok(data.id());
        for series in summary {
            if series.points < 5
                || series.span_minutes() < i64::from(thresholds.sustained_minutes)
                || series.over_fraction() < 0.75
            {
                continue;
            }
            let subject = if series.instance.is_empty() {
                what.to_string()
            } else {
                series.instance.clone()
            };
            report.found(
                Finding::new(
                    check,
                    data.id(),
                    subject.clone(),
                    format!(
                        "{} {} has been over {threshold:.0}{unit} for {} minutes (average {:.0}{unit}, peak {:.0}{unit})",
                        data.name(),
                        subject,
                        series.span_minutes(),
                        series.avg,
                        series.max
                    ),
                )
                .detail(detail)
                .evidence(
                    "Window",
                    format!(
                        "{} samples over {} minutes, {:.0} % of them above {threshold:.0}{unit}",
                        series.points,
                        series.span_minutes(),
                        series.over_fraction() * 100.0
                    ),
                )
                .evidence("Peak", format!("{:.1}{unit}", series.max))
                .evidence("Average", format!("{:.1}{unit}", series.avg)),
            );
        }
    }
    skip_rest(world, report, "this metric is not polled for the device");
}

pub fn high_cpu(world: &World, settings: &Settings, report: &mut CheckReport) {
    sustained(
        world,
        settings,
        report,
        "high_cpu",
        "cpu.pct",
        settings.thresholds.cpu_pct,
        " %",
        "CPU",
        "A control plane this busy starts to lose the things that keep the network converged: \
         routing hellos, spanning-tree frames, ARP replies and the management session you would \
         use to look into it. The cause is usually one process — a loop flooding the CPU, an ACL \
         being processed in software, or SNMP polling that is too aggressive.",
    );
}

pub fn high_memory(world: &World, settings: &Settings, report: &mut CheckReport) {
    sustained(
        world,
        settings,
        report,
        "high_memory",
        "mem.pct",
        settings.thresholds.memory_pct,
        " %",
        "memory",
        "A device that runs out of memory stops accepting new sessions, drops routing adjacencies \
         and on some platforms stops forwarding. On most platforms memory use climbs and does not \
         come back down until the device is restarted, so this is worth planning for rather than \
         waiting on.",
    );
}

pub fn high_temperature(world: &World, settings: &Settings, report: &mut CheckReport) {
    sustained(
        world,
        settings,
        report,
        "high_temperature",
        "temp.c",
        settings.thresholds.temperature_c,
        " °C",
        "temperature",
        "Heat is the most reliable predictor of hardware failure there is, and the causes are \
         almost always physical: a blocked intake, a failed fan, a cupboard being used as a comms \
         room, or a stack packed too tightly.",
    );
}

#[cfg(test)]
#[path = "fleet_tests.rs"]
mod tests;

//! Turning "where" into a list of devices.
//!
//! A task names a filter rather than a list of device ids, and the filter is
//! resolved every time the task runs. That is the whole point: "every access
//! switch in Oslo" keeps meaning that as switches are racked and
//! decommissioned, and nobody has to remember to edit a list.

use nocturne_app::error::{AppError, AppResult};
use nocturne_app::model::Device;
use nocturne_app::{App, inventory};

use crate::model::{Filter, Selector};

/// Resolve a selector against the inventory as it is right now.
///
/// Explicit ids and a filter combine as a union, then `exclude_device_ids` is
/// taken away. The result is sorted by name and free of duplicates. An id that
/// names no device is an error — a typo must not quietly shrink a run.
pub fn resolve(app: &App, selector: &Selector) -> AppResult<Vec<Device>> {
    if selector.is_empty() {
        return Err(AppError::invalid(
            "name the devices (`device_ids`) or how to find them (`filter`)",
        ));
    }
    let mut chosen: Vec<Device> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let push = |device: Device, seen: &mut Vec<String>, chosen: &mut Vec<Device>| {
        if !seen.contains(&device.id) {
            seen.push(device.id.clone());
            chosen.push(device);
        }
    };

    for reference in &selector.device_ids {
        let device = inventory::find(app, reference)?;
        push(device, &mut seen, &mut chosen);
    }
    if let Some(filter) = &selector.filter {
        for device in by_filter(app, filter)? {
            push(device, &mut seen, &mut chosen);
        }
    }

    if !selector.exclude_device_ids.is_empty() {
        let mut excluded = Vec::new();
        for reference in &selector.exclude_device_ids {
            // An exclusion that matches nothing is not an error: the device it
            // named may well have been removed, which is what was wanted.
            if let Ok(device) = inventory::find(app, reference) {
                excluded.push(device.id);
            }
        }
        chosen.retain(|device| !excluded.contains(&device.id));
    }

    if chosen.is_empty() {
        return Err(AppError::not_found(
            "any device matching this selection — nothing would run",
        ));
    }
    if let Some(limit) = selector.filter.as_ref().and_then(|filter| filter.limit) {
        chosen.truncate(limit.max(1));
    }
    Ok(chosen)
}

/// Devices matching a filter, in inventory order (by name).
fn by_filter(app: &App, filter: &Filter) -> AppResult<Vec<Device>> {
    let core = inventory::DeviceFilter {
        query: filter.query.clone(),
        site: filter.site.clone(),
        role: filter.role.clone(),
        tag: filter.tag.clone(),
        profile: filter.profile.clone(),
        status: filter.status.clone(),
        lab: filter.lab,
    };
    Ok(inventory::list(app, &core)?
        .into_iter()
        .filter(|row| match filter.protected {
            Some(protected) => row.device.protected == protected,
            None => true,
        })
        .filter(|row| {
            filter.tags.iter().all(|wanted| {
                row.device
                    .tags
                    .iter()
                    .any(|tag| tag.eq_ignore_ascii_case(wanted))
            })
        })
        .map(|row| row.device)
        .collect())
}

/// A one-line description of what a selector picked, for a job title.
pub fn describe(devices: &[Device]) -> String {
    match devices {
        [] => "no devices".to_string(),
        [one] => one.name.clone(),
        [first, second] => format!("{} and {}", first.name, second.name),
        [first, ..] => format!("{} and {} others", first.name, devices.len() - 1),
    }
}

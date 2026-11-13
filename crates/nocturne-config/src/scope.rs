//! Which devices a request is about, and how their configuration is written.

use std::collections::HashSet;
use std::sync::Arc;

use nocturne_app::inventory::{self, DeviceFilter};
use nocturne_app::model::Device;
use nocturne_app::{App, AppResult, profiles};
use nocturne_drivers::hier::Syntax;
use nocturne_drivers::profile::Platform;

/// A device together with the device type it is driven with.
#[derive(Clone, Debug)]
pub(crate) struct Target {
    pub device: Device,
    /// The device's own profile, or the one detection settled on. Empty when neither is known.
    pub profile: String,
}

impl Target {
    pub fn platform(&self, app: &App) -> Option<Arc<Platform>> {
        profiles::registry(app).get(&self.profile)
    }

    /// The configuration grammar, `flat` (no structure assumed) when the device type is unknown.
    pub fn syntax(&self, app: &App) -> Syntax {
        self.platform(app)
            .map_or(Syntax::Flat, |platform| platform.profile.config.syntax)
    }

    /// The profile and those it extends, nearest first.
    pub fn profile_lineage(&self, app: &App) -> Vec<String> {
        let registry = profiles::registry(app);
        let mut lineage = vec![self.profile.clone()];
        let mut next = registry.get(&self.profile);
        while let Some(platform) = next {
            let Some(parent) = platform.profile.extends.clone() else {
                break;
            };
            // `extends` cannot loop in a loaded registry; stay finite even if it could.
            if lineage.contains(&parent) {
                break;
            }
            next = registry.get(&parent);
            lineage.push(parent);
        }
        lineage
    }
}

fn effective_profile(device: &Device, detected: Option<String>) -> String {
    if device.profile.is_empty() {
        detected.unwrap_or_default()
    } else {
        device.profile.clone()
    }
}

/// One device, by id, name or address.
pub(crate) fn target(app: &App, reference: &str) -> AppResult<Target> {
    let device = inventory::find(app, reference)?;
    let detected = if device.profile.is_empty() {
        inventory::state_of(app, &device.id)?.detected_profile
    } else {
        None
    };
    Ok(Target {
        profile: effective_profile(&device, detected),
        device,
    })
}

/// The devices a request names: those of `device_ids` (ids, names or
/// addresses) that also pass `filter`; every device passing `filter` when no
/// ids are given. In name order.
pub(crate) fn targets(
    app: &App,
    device_ids: Option<&[String]>,
    filter: Option<&DeviceFilter>,
) -> AppResult<Vec<Target>> {
    let wanted: Option<HashSet<String>> = device_ids
        .map(|ids| {
            ids.iter()
                .map(|id| inventory::find(app, id).map(|device| device.id))
                .collect::<AppResult<_>>()
        })
        .transpose()?;
    let everything = DeviceFilter::default();
    Ok(inventory::list(app, filter.unwrap_or(&everything))?
        .into_iter()
        .filter(|row| wanted.as_ref().is_none_or(|ids| ids.contains(&row.device.id)))
        .map(|row| Target {
            profile: effective_profile(&row.device, row.state.detected_profile),
            device: row.device,
        })
        .collect())
}

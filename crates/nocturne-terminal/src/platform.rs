//! Which device profile applies to a terminal target.

use std::sync::Arc;

use nocturne_app::connect::GENERIC_PROFILE;
use nocturne_app::model::Device;
use nocturne_app::{App, AppResult, inventory, profiles};
use nocturne_drivers::profile::Platform;

/// The profile a device is driven with: its own, else the one detection
/// remembered, else the generic one. `Ok(None)` when not even that exists;
/// an error when the device names a profile that is gone.
pub(crate) fn of_device(app: &App, device: &Device) -> AppResult<Option<Arc<Platform>>> {
    if !device.profile.is_empty() {
        return profiles::platform(app, &device.profile).map(Some);
    }
    let registry = profiles::registry(app);
    let detected = inventory::state_of(app, &device.id)?
        .detected_profile
        .and_then(|id| registry.get(&id));
    Ok(detected.or_else(|| registry.get(GENERIC_PROFILE)))
}

/// The profile for targets nothing is known about.
pub(crate) fn generic(app: &App) -> Option<Arc<Platform>> {
    profiles::registry(app).get(GENERIC_PROFILE)
}

/// Extra redaction rules of the profile of a device, for hiding secrets in
/// its output. Empty when the device or its profile cannot be found: the
/// built-in rules then stand alone.
pub(crate) fn secret_rules(app: &App, device_id: Option<&str>) -> Vec<String> {
    device_id
        .and_then(|id| inventory::get(app, id).ok())
        .and_then(|device| of_device(app, &device).ok().flatten())
        .map(|platform| platform.profile.config.secrets.clone())
        .unwrap_or_default()
}
